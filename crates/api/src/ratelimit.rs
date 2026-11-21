//! 로그인 실패 rate limit (프로세스 메모리).
//!
//! API 서버가 한 대인 MVP 구성을 전제로 한다 (여러 대로 늘리면 Redis 등 공유 저장소로 옮긴다).
//!
//! ## 키
//! - `Email(sha256(정규화 이메일))`: 계정 단위 상한 `max_per_email`
//! - `Ip(ip)`: 클라이언트 단위 상한 `max_per_ip`
//! - `EmailIp(sha256, ip)`: 그 IP가 그 계정에 실패한 기록 (잠금 우회 판정용)
//! - `Bypass(sha256)`: 계정이 잠긴 뒤 "깨끗한 IP" 예외로 허용한 시도 수 (`max_bypass_per_email`)
//!
//! 이메일 원문은 저장하지 않는다 (키 크기가 입력 길이와 무관하게 고정된다).
//!
//! ## 동시 요청 (예약)
//! [`LoginLimiter::reserve`]는 상한 검사와 "진행 중 시도(pending)" 기록을 한 번의 잠금 안에서
//! 한다. 진행 중 시도는 실패로 간주해 세므로, 동시에 N개를 보내도 상한 이상은 비밀번호
//! 검증까지 가지 못한다. 결과가 나오면 [`Reservation::fail`](실패로 전환) 또는
//! [`Reservation::succeed`](해제 + 이메일 기록 초기화)를 부르고, 둘 다 부르지 않고 drop되면
//! (DB 오류 등) 예약만 해제한다.
//!
//! ## 계정 잠금 DoS 완화
//! 이메일 상한에 닿아도, IP를 알고 그 IP가 창 안에서 이 계정에 실패(진행 중 포함)한 적이 없으면
//! 시도를 허용한다. 공격자가 IP를 바꿔 가며 이 예외를 무한히 쓰지 못하게 계정당
//! `max_bypass_per_email`회(창 단위)로 제한한다. 즉 IP를 돌리는 공격자가 한 창에서 시도할 수
//! 있는 횟수는 최대 `max_per_email + max_bypass_per_email`이고, 그 예산을 다 쓰기 전까지는
//! 실패 기록이 없는 IP의 정상 사용자가 로그인할 수 있다.
//!
//! ## 메모리
//! 키 수는 `max_keys`로 제한한다. 새 키를 넣을 때 맵이 가득 차면, 마지막 정리 이후 삽입이
//! 충분히 쌓였을 때만 만료 항목을 정리하고(분할 상환 O(1)), 그래도 가득 차면 가장 먼저 만든
//! 키부터 내보낸다.

use crate::config::LoginLimitConfig;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 이메일 키: 정규화한 이메일의 sha256.
pub type EmailHash = [u8; 32];

pub fn email_hash(normalized_email: &str) -> EmailHash {
    Sha256::digest(normalized_email.as_bytes()).into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Email(EmailHash),
    Ip(IpAddr),
    EmailIp(EmailHash, IpAddr),
    Bypass(EmailHash),
}

#[derive(Debug, Default)]
struct Entry {
    /// 창 안의 실패 시각 (오래된 것부터). 길이는 상한까지만 유지한다.
    failures: VecDeque<Instant>,
    /// 결과를 기다리는 시도 수.
    pending: u32,
}

impl Entry {
    fn expire(&mut self, now: Instant, window: Duration) {
        while self
            .failures
            .front()
            .is_some_and(|t| now.duration_since(*t) >= window)
        {
            self.failures.pop_front();
        }
    }

    fn count(&self) -> usize {
        self.failures.len() + self.pending as usize
    }

    fn is_idle(&self) -> bool {
        self.failures.is_empty() && self.pending == 0
    }
}

#[derive(Default)]
struct Inner {
    map: HashMap<Key, Entry>,
    /// 키 생성 순서 (내보내기용). 이미 지워진 키가 섞여 있을 수 있다.
    order: VecDeque<Key>,
    inserts_since_prune: usize,
}

pub struct LoginLimiter {
    cfg: LoginLimitConfig,
    inner: Mutex<Inner>,
}

/// 예약된 로그인 시도. [`fail`](Self::fail)·[`succeed`](Self::succeed) 없이 drop되면 해제만 한다.
pub struct Reservation {
    limiter: Arc<LoginLimiter>,
    keys: Vec<Key>,
    done: bool,
}

impl Reservation {
    /// 실패로 전환한다.
    pub fn fail(mut self) {
        self.done = true;
        self.limiter.finish(&self.keys, true);
    }

    /// 성공: 예약을 해제하고 이 이메일의 실패 기록(이메일·예외 예산·이 IP와의 조합)을 지운다.
    /// IP 기록은 유지한다 (공격자가 자기 계정으로 성공해 IP 제한을 초기화하지 못하게).
    pub fn succeed(mut self) {
        self.done = true;
        self.limiter.finish(&self.keys, false);
        let mut inner = self.limiter.lock();
        for k in &self.keys {
            if matches!(k, Key::Email(_) | Key::Bypass(_) | Key::EmailIp(..)) {
                inner.map.remove(k);
            }
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.done {
            self.limiter.finish(&self.keys, false);
        }
    }
}

impl LoginLimiter {
    pub fn new(cfg: LoginLimitConfig) -> Self {
        Self {
            cfg,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 현재 키 수 (테스트·모니터링용).
    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 상한 검사와 예약을 한 번에 한다. 제한에 걸렸으면 `Err(재시도까지 남은 초)`.
    pub fn reserve(
        self: &Arc<Self>,
        email: &EmailHash,
        ip: Option<IpAddr>,
    ) -> Result<Reservation, u64> {
        let now = Instant::now();
        let window = self.cfg.window;
        let mut inner = self.lock();

        let mut probe = |key: Key| -> (usize, Option<Duration>) {
            match inner.map.get_mut(&key) {
                Some(e) => {
                    e.expire(now, window);
                    let wait = e
                        .failures
                        .front()
                        .map(|t| window.saturating_sub(now.duration_since(*t)));
                    (e.count(), wait)
                }
                None => (0, None),
            }
        };
        // 실패 기록 없이 진행 중 시도만으로 막혔으면 곧 풀린다.
        let wait_secs = |w: Option<Duration>| w.map_or(1, |d| d.as_secs() + 1);

        let email_key = Key::Email(*email);
        let mut keys = vec![email_key];
        if let Some(ip) = ip {
            let (n, w) = probe(Key::Ip(ip));
            if n >= self.cfg.max_per_ip as usize {
                return Err(wait_secs(w));
            }
            keys.push(Key::Ip(ip));
            keys.push(Key::EmailIp(*email, ip));
        }
        let (n, w) = probe(email_key);
        if n >= self.cfg.max_per_email as usize {
            // 잠긴 계정: 이 계정에 실패한 적 없는 IP라면 예산 안에서 허용한다.
            let Some(ip) = ip else {
                return Err(wait_secs(w));
            };
            let (pair, _) = probe(Key::EmailIp(*email, ip));
            let (used, _) = probe(Key::Bypass(*email));
            if pair > 0 || used >= self.cfg.max_bypass_per_email as usize {
                return Err(wait_secs(w));
            }
            keys.push(Key::Bypass(*email));
        }
        for k in &keys {
            self.entry(&mut inner, *k, now).pending += 1;
        }
        drop(inner);
        Ok(Reservation {
            limiter: Arc::clone(self),
            keys,
            done: false,
        })
    }

    fn finish(&self, keys: &[Key], failed: bool) {
        let now = Instant::now();
        let cap = self
            .cfg
            .max_per_email
            .max(self.cfg.max_per_ip)
            .max(self.cfg.max_bypass_per_email) as usize;
        let mut inner = self.lock();
        for k in keys {
            if let Some(e) = inner.map.get_mut(k) {
                e.pending = e.pending.saturating_sub(1);
            }
            if failed {
                let e = self.entry(&mut inner, *k, now);
                e.expire(now, self.cfg.window);
                e.failures.push_back(now);
                // 상한보다 많이 들고 있을 필요 없다.
                while e.failures.len() > cap.max(1) {
                    e.failures.pop_front();
                }
            } else if inner.map.get(k).is_some_and(Entry::is_idle) {
                inner.map.remove(k);
            }
        }
    }

    /// 키의 항목. 없으면 만들고, 맵이 가득 찼으면 정리·내보내기로 자리를 만든다.
    fn entry<'a>(&self, inner: &'a mut Inner, key: Key, now: Instant) -> &'a mut Entry {
        if !inner.map.contains_key(&key) {
            let max_keys = self.cfg.max_keys.max(1);
            if inner.map.len() >= max_keys {
                // 분할 상환: 정리는 삽입이 max_keys/4 이상 쌓였을 때만 한다.
                if inner.inserts_since_prune >= (max_keys / 4).max(1) {
                    self.prune(inner, now);
                }
                while inner.map.len() >= max_keys {
                    match inner.order.pop_front() {
                        Some(old) => {
                            inner.map.remove(&old);
                        }
                        None => break,
                    }
                }
            }
            inner.order.push_back(key);
            inner.inserts_since_prune += 1;
            // 지워진 키가 순서 큐에 계속 쌓이지 않게 한다.
            if inner.order.len() > max_keys.saturating_mul(2) {
                let Inner { map, order, .. } = &mut *inner;
                order.retain(|k| map.contains_key(k));
            }
        }
        inner.map.entry(key).or_default()
    }

    fn prune(&self, inner: &mut Inner, now: Instant) {
        let window = self.cfg.window;
        let Inner {
            map,
            order,
            inserts_since_prune,
        } = inner;
        map.retain(|_, e| {
            e.expire(now, window);
            !e.is_idle()
        });
        order.retain(|k| map.contains_key(k));
        *inserts_since_prune = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(window_ms: u64) -> Arc<LoginLimiter> {
        Arc::new(LoginLimiter::new(LoginLimitConfig {
            max_per_email: 3,
            max_per_ip: 5,
            max_bypass_per_email: 2,
            window: Duration::from_millis(window_ms),
            max_keys: 1_000,
        }))
    }

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    fn fail(l: &Arc<LoginLimiter>, email: &str, ip: Option<IpAddr>) {
        l.reserve(&email_hash(email), ip).expect("허용").fail();
    }

    #[test]
    fn blocks_after_max_failures_per_email() {
        let l = limiter(60_000);
        for _ in 0..3 {
            fail(&l, "a@x", None);
        }
        let wait = l.reserve(&email_hash("a@x"), None).err().unwrap();
        assert!((1..=61).contains(&wait), "{wait}");
        assert!(
            l.reserve(&email_hash("b@x"), None).is_ok(),
            "다른 이메일은 영향 없음"
        );
    }

    #[test]
    fn pending_attempts_count_toward_the_limit() {
        let l = limiter(60_000);
        let h = email_hash("a@x");
        let held: Vec<_> = (0..3).map(|_| l.reserve(&h, None).unwrap()).collect();
        assert!(l.reserve(&h, None).is_err(), "진행 중 3개면 4번째는 거부");
        drop(held);
        assert!(l.reserve(&h, None).is_ok(), "해제되면 다시 허용");
        assert!(l.is_empty(), "해제 후 빈 항목은 지운다");
    }

    #[test]
    fn blocks_per_ip_across_emails() {
        let l = limiter(60_000);
        for i in 0..5 {
            fail(&l, &format!("u{i}@x"), ip("1.2.3.4"));
        }
        assert!(l.reserve(&email_hash("new@x"), ip("1.2.3.4")).is_err());
        assert!(l.reserve(&email_hash("new@x"), ip("5.6.7.8")).is_ok());
        assert!(l.reserve(&email_hash("new@x"), None).is_ok());
    }

    #[test]
    fn window_expiry_unblocks() {
        let l = limiter(50);
        for _ in 0..3 {
            fail(&l, "a@x", None);
        }
        assert!(l.reserve(&email_hash("a@x"), None).is_err());
        std::thread::sleep(Duration::from_millis(80));
        assert!(l.reserve(&email_hash("a@x"), None).is_ok());
    }

    #[test]
    fn success_resets_email_but_not_ip() {
        let l = limiter(60_000);
        for _ in 0..3 {
            fail(&l, "a@x", ip("1.1.1.1"));
        }
        // 잠긴 계정이지만 깨끗한 IP 예외로 성공
        l.reserve(&email_hash("a@x"), ip("2.2.2.2"))
            .unwrap()
            .succeed();
        assert!(l.reserve(&email_hash("a@x"), None).is_ok());
        fail(&l, "b@x", ip("1.1.1.1"));
        fail(&l, "c@x", ip("1.1.1.1"));
        assert!(l.reserve(&email_hash("z@x"), ip("1.1.1.1")).is_err());
    }

    #[test]
    fn locked_email_allows_clean_ip_within_bypass_budget() {
        let l = limiter(60_000);
        let h = email_hash("a@x");
        for i in 0..3 {
            fail(&l, "a@x", ip(&format!("10.0.0.{i}")));
        }
        // 실패했던 IP·IP 모름은 거부
        assert!(l.reserve(&h, ip("10.0.0.0")).is_err());
        assert!(l.reserve(&h, None).is_err());
        // 깨끗한 IP는 예산(2회)까지 허용
        fail(&l, "a@x", ip("10.0.1.1"));
        fail(&l, "a@x", ip("10.0.1.2"));
        assert!(l.reserve(&h, ip("10.0.1.3")).is_err(), "예산 소진");
    }

    #[test]
    fn memory_is_bounded_under_many_keys() {
        let l = limiter(60_000);
        for i in 0..50_000u32 {
            let addr = IpAddr::from([10, (i >> 16) as u8, (i >> 8) as u8, i as u8]);
            fail(&l, &format!("user{i}@x"), Some(addr));
        }
        assert!(l.len() <= 1_000, "{}", l.len());
        let inner = l.lock();
        assert!(inner.order.len() <= 2_000);
    }
}

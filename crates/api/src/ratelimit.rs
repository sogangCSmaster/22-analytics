//! 로그인 실패 rate limit (프로세스 메모리).
//!
//! 이메일별, 클라이언트 IP별로 창(window) 안의 실패 시각을 기록한다. 상한에 닿으면
//! 비밀번호가 맞아도 429를 돌려준다. API 서버가 한 대인 MVP 구성을 전제로 한다
//! (여러 대로 늘리면 Redis 등 공유 저장소로 옮긴다).
//!
//! IP는 `cf-connecting-ip` → `x-forwarded-for` 첫 값 순서로 읽는다. 이 헤더는 프록시 뒤에서만
//! 믿을 수 있으므로, 위조로 IP 제한을 피하더라도 이메일 제한이 계정을 보호한다.

use crate::config::LoginLimitConfig;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 키가 이만큼 쌓이면 만료된 항목을 정리한다 (위조 IP로 메모리를 채우는 공격 대비).
const PRUNE_AT: usize = 10_000;
/// 정리 후에도 이만큼 넘으면 새 IP 키는 기록하지 않는다.
const HARD_CAP: usize = 100_000;

pub struct LoginLimiter {
    cfg: LoginLimitConfig,
    failures: Mutex<HashMap<String, Vec<Instant>>>,
}

impl LoginLimiter {
    pub fn new(cfg: LoginLimitConfig) -> Self {
        Self {
            cfg,
            failures: Mutex::new(HashMap::new()),
        }
    }

    fn email_key(email: &str) -> String {
        format!("e:{email}")
    }

    fn ip_key(ip: &str) -> String {
        format!("i:{ip}")
    }

    /// 제한에 걸렸으면 `Err(재시도까지 남은 초)`.
    pub fn check(&self, email: &str, ip: Option<&str>) -> Result<(), u64> {
        let now = Instant::now();
        let map = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let mut wait = None::<Duration>;
        let mut probe = |key: String, max: u32| {
            if let Some(ts) = map.get(&key) {
                let recent: Vec<&Instant> = ts
                    .iter()
                    .filter(|t| now.duration_since(**t) < self.cfg.window)
                    .collect();
                if recent.len() >= max as usize {
                    // 가장 오래된 실패가 창 밖으로 나가는 시점까지 기다린다.
                    let oldest = recent.iter().min().map(|t| **t).unwrap_or(now);
                    let w = self.cfg.window.saturating_sub(now.duration_since(oldest));
                    wait = Some(wait.map_or(w, |cur| cur.max(w)));
                }
            }
        };
        probe(Self::email_key(email), self.cfg.max_per_email);
        if let Some(ip) = ip {
            probe(Self::ip_key(ip), self.cfg.max_per_ip);
        }
        match wait {
            Some(w) => Err(w.as_secs() + 1),
            None => Ok(()),
        }
    }

    pub fn record_failure(&self, email: &str, ip: Option<&str>) {
        let now = Instant::now();
        let mut map = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= PRUNE_AT {
            let window = self.cfg.window;
            map.retain(|_, ts| {
                ts.retain(|t| now.duration_since(*t) < window);
                !ts.is_empty()
            });
        }
        let cap = self.cfg.max_per_email.max(self.cfg.max_per_ip) as usize;
        let push = |key: String, map: &mut HashMap<String, Vec<Instant>>| {
            let ts = map.entry(key).or_default();
            ts.retain(|t| now.duration_since(*t) < self.cfg.window);
            ts.push(now);
            // 상한보다 많이 들고 있을 필요 없다.
            if ts.len() > cap {
                let excess = ts.len() - cap;
                ts.drain(..excess);
            }
        };
        push(Self::email_key(email), &mut map);
        if let Some(ip) = ip {
            let key = Self::ip_key(ip);
            if map.len() < HARD_CAP || map.contains_key(&key) {
                push(key, &mut map);
            }
        }
    }

    /// 로그인 성공 시 그 이메일의 실패 기록을 지운다. IP 기록은 유지한다
    /// (공격자가 자기 계정으로 성공해 IP 제한을 초기화하지 못하게).
    pub fn reset_email(&self, email: &str) {
        let mut map = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&Self::email_key(email));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(window_ms: u64) -> LoginLimiter {
        LoginLimiter::new(LoginLimitConfig {
            max_per_email: 3,
            max_per_ip: 5,
            window: Duration::from_millis(window_ms),
        })
    }

    #[test]
    fn blocks_after_max_failures_per_email() {
        let l = limiter(60_000);
        for _ in 0..3 {
            assert!(l.check("a@x", None).is_ok());
            l.record_failure("a@x", None);
        }
        let wait = l.check("a@x", None).unwrap_err();
        assert!((1..=61).contains(&wait), "{wait}");
        assert!(l.check("b@x", None).is_ok(), "다른 이메일은 영향 없음");
    }

    #[test]
    fn blocks_per_ip_across_emails() {
        let l = limiter(60_000);
        for i in 0..5 {
            l.record_failure(&format!("u{i}@x"), Some("1.2.3.4"));
        }
        assert!(l.check("new@x", Some("1.2.3.4")).is_err());
        assert!(l.check("new@x", Some("5.6.7.8")).is_ok());
        assert!(l.check("new@x", None).is_ok());
    }

    #[test]
    fn window_expiry_unblocks() {
        let l = limiter(50);
        for _ in 0..3 {
            l.record_failure("a@x", None);
        }
        assert!(l.check("a@x", None).is_err());
        std::thread::sleep(Duration::from_millis(80));
        assert!(l.check("a@x", None).is_ok());
    }

    #[test]
    fn success_resets_email_but_not_ip() {
        let l = limiter(60_000);
        for _ in 0..3 {
            l.record_failure("a@x", Some("1.1.1.1"));
        }
        l.reset_email("a@x");
        assert!(l.check("a@x", None).is_ok());
        l.record_failure("b@x", Some("1.1.1.1"));
        l.record_failure("c@x", Some("1.1.1.1"));
        assert!(l.check("z@x", Some("1.1.1.1")).is_err());
    }

    #[test]
    fn memory_is_bounded_under_spoofed_ips() {
        let l = limiter(60_000);
        for i in 0..(PRUNE_AT + 50) {
            l.record_failure(
                "a@x",
                Some(&format!("10.{}.{}.{}", i >> 16, (i >> 8) & 255, i & 255)),
            );
        }
        let map = l.failures.lock().unwrap();
        assert!(map.len() <= HARD_CAP);
        assert!(map["e:a@x"].len() <= 5);
    }
}

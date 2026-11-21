//! 원점(ingest) 전달: 서명된 요청 생성, 결과 분류, Queue 메시지와 재시도 정책.
//!
//! 서명은 `analytics_core::edge_auth` v1(내부 경로·타임스탬프·본문 해시·전달 헤더)을 쓴다. 클라이언트 헤더는
//! 하나도 복사하지 않고 매번 새로 만든다. 따라서 클라이언트가 보낸 `x-edge-*` 헤더는 원점에
//! 닿지 않는다.

use analytics_core::Event;
use analytics_core::edge_auth::{self, HEADER_SIGNATURE, HEADER_TIMESTAMP};
use serde::{Deserialize, Serialize};

pub const COLLECT_PATH: &str = edge_auth::COLLECT_PATH;
pub const ORDERS_PATH: &str = edge_auth::ORDERS_PATH;

/// Cloudflare Queue 메시지 한도는 128KB. JSON 래핑 여유를 두고 본문을 이 크기 이하로 나눈다.
pub const MAX_QUEUE_BODY: usize = 100 * 1024;

/// ingest와 같은 기준: 운영 비밀값은 32자 이상.
pub const MIN_SECRET_LEN: usize = 32;

pub fn valid_secret(secret: &str) -> bool {
    secret.len() >= MIN_SECRET_LEN
}

/// 원점으로 보낼 요청. wasm 계층이 `fetch`로 옮긴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl ForwardRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// `{origin}{path}`. ORIGIN_URL 끝의 `/`는 무시한다.
pub fn join_url(origin_url: &str, path: &str) -> String {
    format!("{}{path}", origin_url.trim_end_matches('/'))
}

/// 원점에 전달하는 클라이언트 유래 헤더. 비어 있으면 보내지 않고, 서명에는 `""`로 들어간다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Forwarded<'a> {
    pub path: &'a str,
    pub authorization: &'a str,
    pub idempotency_key: &'a str,
}

pub use analytics_core::orders::api::{
    HEADER_FORWARDED_AUTHORIZATION, HEADER_FORWARDED_PATH, HEADER_IDEMPOTENCY_KEY,
};

/// 서명 헤더를 붙인 원점 요청 (전달 헤더 없음). 본문은 바이트 그대로 서명한다.
pub fn signed(
    origin_url: &str,
    path: &str,
    secret: &[u8],
    body: Vec<u8>,
    now_ms: i64,
) -> ForwardRequest {
    signed_with(origin_url, path, secret, body, now_ms, Forwarded::default())
}

/// 전달 헤더까지 서명한 원점 요청 (v1: 내부 경로·타임스탬프·본문 해시·전달 헤더).
pub fn signed_with(
    origin_url: &str,
    path: &str,
    secret: &[u8],
    body: Vec<u8>,
    now_ms: i64,
    fwd: Forwarded<'_>,
) -> ForwardRequest {
    let signature = edge_auth::sign(
        secret,
        &edge_auth::SignedRequest {
            internal_path: path,
            timestamp_ms: now_ms,
            body: &body,
            forwarded_path: fwd.path,
            forwarded_authorization: fwd.authorization,
            idempotency_key: fwd.idempotency_key,
        },
    );
    let mut headers: Vec<(String, String)> = vec![
        ("content-type".into(), "application/json".into()),
        (HEADER_TIMESTAMP.into(), now_ms.to_string()),
        (HEADER_SIGNATURE.into(), signature),
    ];
    for (name, v) in [
        (HEADER_FORWARDED_AUTHORIZATION, fwd.authorization),
        (HEADER_FORWARDED_PATH, fwd.path),
        (HEADER_IDEMPOTENCY_KEY, fwd.idempotency_key),
    ] {
        if !v.is_empty() {
            headers.push((name.into(), v.to_owned()));
        }
    }
    ForwardRequest {
        url: join_url(origin_url, path),
        headers,
        body,
    }
}

const EMPTY_BODY: &str = r#"{"events":[]}"#;

/// `/internal/collect` 본문 `{"events":[...]}`.
pub fn collect_body(events: &[Event]) -> String {
    #[derive(Serialize)]
    struct Body<'a> {
        events: &'a [Event],
    }
    // Event는 문자열·숫자·UUID뿐이라 직렬화가 실패하지 않는다.
    serde_json::to_string(&Body { events }).unwrap_or_else(|_| EMPTY_BODY.to_owned())
}

/// 원점 응답 분류. `None`은 네트워크 오류·타임아웃.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Delivered,
    /// 재시도해도 결과가 같은 거절 (4xx). 로그를 남기고 버린다.
    Rejected(u16),
    /// 일시 장애 (네트워크, 타임아웃, 5xx, 408, 429)
    Retry,
}

pub fn classify(status: Option<u16>) -> Delivery {
    match status {
        Some(200..=299) => Delivery::Delivered,
        Some(408 | 429) | None => Delivery::Retry,
        Some(s @ 400..=499) => Delivery::Rejected(s),
        Some(_) => Delivery::Retry,
    }
}

/// Queue에 넣는 메시지. 서명은 넣지 않는다: 타임스탬프 허용 오차(5분)보다 오래 머물 수 있으므로
/// 소비할 때마다 같은 본문을 새 타임스탬프로 다시 서명한다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedForward {
    /// 메시지 형식 버전
    pub v: u8,
    /// `/internal/collect` 본문 JSON 그대로
    pub body: String,
}

pub const QUEUE_MESSAGE_VERSION: u8 = 1;

impl QueuedForward {
    pub fn to_request(&self, origin_url: &str, secret: &[u8], now_ms: i64) -> ForwardRequest {
        signed(
            origin_url,
            COLLECT_PATH,
            secret,
            self.body.as_bytes().to_vec(),
            now_ms,
        )
    }
}

/// 이벤트를 Queue 메시지들로 나눈다. 각 메시지 본문은 `MAX_QUEUE_BODY` 이하
/// (이벤트 1건이 한도를 넘는 경우는 그 1건만 담는다). 이벤트 순서와 내용은 그대로다.
pub fn queue_messages(events: &[Event]) -> Vec<QueuedForward> {
    let mut out = Vec::new();
    let mut start = 0;
    // 압축 JSON 본문 길이 = 래퍼 + Σ이벤트 + 쉼표(n-1)
    let mut len = EMPTY_BODY.len();
    for (i, e) in events.iter().enumerate() {
        let size = serde_json::to_string(e).map_or(0, |s| s.len());
        let add = size + usize::from(i > start);
        if i > start && len + add > MAX_QUEUE_BODY {
            out.push(message(&events[start..i]));
            start = i;
            len = EMPTY_BODY.len() + size;
        } else {
            len += add;
        }
    }
    if start < events.len() {
        out.push(message(&events[start..]));
    }
    out
}

fn message(events: &[Event]) -> QueuedForward {
    QueuedForward {
        v: QUEUE_MESSAGE_VERSION,
        body: collect_body(events),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// 원점이 4xx로 거절
    Rejected(u16),
    /// 최대 시도 횟수 초과
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueAction {
    Ack,
    Retry {
        delay_secs: u32,
    },
    /// ack하고 버린다. wasm 계층이 사유를 로그로 남긴다.
    Drop(DropReason),
}

/// Queue consumer 재시도 정책: 지수 백오프, 최대 시도 횟수.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_secs: u32,
    pub max_delay_secs: u32,
}

impl Default for RetryPolicy {
    /// 30초부터 2배씩, 최대 1시간 간격, 10회 (대기 합계 약 3시간 동안 원점 장애를 버틴다).
    fn default() -> Self {
        Self {
            max_attempts: 10,
            base_delay_secs: 30,
            max_delay_secs: 3600,
        }
    }
}

impl RetryPolicy {
    /// `attempts`: 이번 전달이 몇 번째 시도인지 (Queue 규약대로 1부터).
    pub fn decide(&self, delivery: Delivery, attempts: u32) -> QueueAction {
        match delivery {
            Delivery::Delivered => QueueAction::Ack,
            Delivery::Rejected(s) => QueueAction::Drop(DropReason::Rejected(s)),
            Delivery::Retry if attempts >= self.max_attempts => {
                QueueAction::Drop(DropReason::Exhausted)
            }
            Delivery::Retry => QueueAction::Retry {
                delay_secs: self.delay_after(attempts),
            },
        }
    }

    /// `attempts`번째 시도가 실패한 뒤 기다릴 시간.
    pub fn delay_after(&self, attempts: u32) -> u32 {
        let exp = attempts.saturating_sub(1).min(31);
        self.base_delay_secs
            .saturating_mul(1u32 << exp)
            .min(self.max_delay_secs)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use analytics_core::edge_auth::{SignedRequest, verify};
    use uuid::Uuid;

    pub(crate) const SECRET: &[u8] = b"edge-shared-secret";
    pub(crate) const NOW: i64 = 1_763_631_912_000;

    pub(crate) fn event(n: u128) -> Event {
        Event {
            project_id: 7,
            event_id: Uuid::from_u128(n),
            ts_ms: NOW,
            received_at_ms: NOW,
            event_name: "page_view".into(),
            visitor_id: Uuid::from_u128(1000),
            session_id: Uuid::from_u128(2000),
            is_session_start: true,
            member_hash: String::new(),
            url: "https://www.client.com/".into(),
            host: "www.client.com".into(),
            path: "/".into(),
            title: String::new(),
            referrer: String::new(),
            referrer_host: String::new(),
            utm_source: String::new(),
            utm_medium: String::new(),
            utm_campaign: String::new(),
            utm_content: String::new(),
            utm_term: String::new(),
            click_id: String::new(),
            click_id_type: String::new(),
            channel: "direct".into(),
            country: "KR".into(),
            region: String::new(),
            city: String::new(),
            device_type: "desktop".into(),
            browser: "Chrome".into(),
            os: "Windows 10".into(),
            screen_w: 1280,
            props: vec![],
            ec: None,
        }
    }

    /// 원점(ingest)이 하는 것과 같이 요청 헤더로 서명 대상을 다시 만들어 검증한다.
    pub(crate) fn origin_verifies(
        req: &ForwardRequest,
        internal_path: &str,
        body: &[u8],
        now: i64,
    ) -> bool {
        let ts: i64 = req.header(HEADER_TIMESTAMP).unwrap().parse().unwrap();
        let signed = SignedRequest {
            internal_path,
            timestamp_ms: ts,
            body,
            forwarded_path: req.header("x-forwarded-path").unwrap_or(""),
            forwarded_authorization: req.header("x-forwarded-authorization").unwrap_or(""),
            idempotency_key: req.header("idempotency-key").unwrap_or(""),
        };
        verify(SECRET, &signed, req.header(HEADER_SIGNATURE).unwrap(), now)
    }

    fn verifies(req: &ForwardRequest, now: i64) -> bool {
        req.url.ends_with(COLLECT_PATH) && origin_verifies(req, COLLECT_PATH, &req.body, now)
    }

    // ── 해피 ──
    #[test]
    fn signed_forward_verifies_with_core() {
        let body = collect_body(&[event(1), event(2)]);
        let req = signed(
            "https://origin.internal",
            COLLECT_PATH,
            SECRET,
            body.clone().into_bytes(),
            NOW,
        );
        assert_eq!(req.url, "https://origin.internal/internal/collect");
        assert_eq!(req.header(HEADER_TIMESTAMP), Some(NOW.to_string().as_str()));
        assert_eq!(req.header("content-type"), Some("application/json"));
        assert!(verifies(&req, NOW));
        // ingest `/internal/collect`의 EdgeForward와 같은 모양
        let v: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        let evs: Vec<Event> = serde_json::from_value(v["events"].clone()).unwrap();
        assert_eq!(evs, vec![event(1), event(2)]);
    }

    #[test]
    fn join_url_trims_trailing_slash() {
        assert_eq!(
            join_url("https://o.x/", "/internal/collect"),
            "https://o.x/internal/collect"
        );
        assert_eq!(
            join_url("https://o.x", "/internal/orders"),
            "https://o.x/internal/orders"
        );
    }

    // ── 외부 공격: 서명은 본문 전체를 정확히 덮는다 ──
    #[test]
    fn tampering_breaks_signature() {
        let req = signed(
            "https://o",
            COLLECT_PATH,
            SECRET,
            collect_body(&[event(1)]).into_bytes(),
            NOW,
        );
        let sig = req.header(HEADER_SIGNATURE).unwrap();
        let c = |ts, body| SignedRequest::collect(ts, body);
        assert!(verify(SECRET, &c(NOW, &req.body), sig, NOW));
        let mut tampered = req.body.clone();
        let i = tampered.len() / 2;
        tampered[i] ^= 1;
        assert!(!verify(SECRET, &c(NOW, &tampered), sig, NOW));
        let mut extended = req.body.clone();
        extended.push(b' ');
        assert!(!verify(SECRET, &c(NOW, &extended), sig, NOW));
        assert!(!verify(
            SECRET,
            &c(NOW, &req.body[..req.body.len() - 1]),
            sig,
            NOW
        ));
        assert!(!verify(b"wrong", &c(NOW, &req.body), sig, NOW));
        assert!(
            !verify(SECRET, &c(NOW + 1, &req.body), sig, NOW),
            "타임스탬프도 서명에 포함"
        );
    }

    #[test]
    fn collect_signature_is_bound_to_collect_path() {
        let req = signed("https://o", COLLECT_PATH, SECRET, b"{}".to_vec(), NOW);
        assert!(origin_verifies(&req, COLLECT_PATH, &req.body, NOW));
        assert!(
            !origin_verifies(&req, ORDERS_PATH, &req.body, NOW),
            "다른 내부 경로로 재생할 수 없다"
        );
    }

    #[test]
    fn requeued_message_resigned_in_v1_format() {
        let m = &queue_messages(&[event(1)])[0];
        let r = m.to_request("https://o", SECRET, NOW + 60_000);
        assert!(origin_verifies(&r, COLLECT_PATH, &r.body, NOW + 60_000));
    }

    #[test]
    fn only_edge_generated_headers() {
        let req = signed("https://o", COLLECT_PATH, SECRET, b"{}".to_vec(), NOW);
        let mut names: Vec<_> = req.headers.iter().map(|(k, _)| k.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            vec!["content-type", HEADER_SIGNATURE, HEADER_TIMESTAMP]
        );
    }

    // ── 멱등성 ──
    #[test]
    fn requeued_payload_is_identical_and_resigned() {
        let msgs = queue_messages(&[event(1), event(2)]);
        assert_eq!(msgs.len(), 1);
        let m = &msgs[0];
        // JSON 왕복 (Queue 직렬화) 후에도 같다
        let back: QueuedForward = serde_json::from_str(&serde_json::to_string(m).unwrap()).unwrap();
        assert_eq!(&back, m);

        let first = m.to_request("https://o", SECRET, NOW);
        let later = back.to_request("https://o", SECRET, NOW + 30 * 60 * 1000);
        assert_eq!(first.body, later.body, "본문은 바이트 단위로 같다");
        assert_eq!(first.url, "https://o/internal/collect");
        assert!(verifies(&first, NOW));
        assert!(
            verifies(&later, NOW + 30 * 60 * 1000),
            "늦은 재시도도 새 타임스탬프로 검증된다"
        );
        // 같은 시각에 다시 서명하면 서명도 같다 (결정적)
        assert_eq!(first, m.to_request("https://o", SECRET, NOW));
    }

    #[test]
    fn duplicate_event_ids_pass_through_for_downstream_dedupe() {
        let dup = vec![event(1), event(1), event(2)];
        let body = collect_body(&dup);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let ids: Vec<_> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["event_id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], ids[1]);
    }

    // ── 경계값: Queue 메시지 크기 ──
    #[test]
    fn large_batches_are_split_for_queue() {
        let mut big = Vec::new();
        for i in 0..50 {
            let mut e = event(i);
            e.props = (0..50).map(|p| (format!("k{p}"), "v".repeat(60))).collect();
            big.push(e);
        }
        let whole = collect_body(&big);
        assert!(whole.len() > MAX_QUEUE_BODY, "테스트 전제: {}", whole.len());
        let msgs = queue_messages(&big);
        assert!(msgs.len() > 1);
        let mut all = Vec::new();
        for m in &msgs {
            assert!(m.body.len() <= MAX_QUEUE_BODY, "{}", m.body.len());
            assert_eq!(m.v, QUEUE_MESSAGE_VERSION);
            let v: serde_json::Value = serde_json::from_str(&m.body).unwrap();
            let evs: Vec<Event> = serde_json::from_value(v["events"].clone()).unwrap();
            all.extend(evs);
        }
        assert_eq!(all, big, "순서·내용 보존");
        assert!(queue_messages(&[]).is_empty());
    }

    #[test]
    fn single_oversized_event_still_queued_alone() {
        let mut e = event(1);
        e.props = vec![("k".into(), "x".repeat(MAX_QUEUE_BODY + 10))];
        let msgs = queue_messages(&[event(0), e.clone(), event(2)]);
        assert_eq!(msgs.len(), 3);
        assert!(msgs[1].body.contains(&"x".repeat(100)));
    }

    // ── 결과 분류 ──
    #[test]
    fn classify_origin_status() {
        assert_eq!(classify(Some(204)), Delivery::Delivered);
        assert_eq!(classify(Some(200)), Delivery::Delivered);
        assert_eq!(classify(Some(400)), Delivery::Rejected(400));
        assert_eq!(classify(Some(401)), Delivery::Rejected(401));
        assert_eq!(classify(Some(408)), Delivery::Retry);
        assert_eq!(classify(Some(429)), Delivery::Retry);
        assert_eq!(classify(Some(500)), Delivery::Retry);
        assert_eq!(classify(Some(503)), Delivery::Retry);
        assert_eq!(classify(Some(530)), Delivery::Retry, "Tunnel 오류");
        assert_eq!(classify(Some(302)), Delivery::Retry);
        assert_eq!(classify(None), Delivery::Retry, "네트워크 오류·타임아웃");
    }

    #[test]
    fn secret_length_matches_ingest_rule() {
        assert!(!valid_secret(""));
        assert!(!valid_secret(&"s".repeat(31)));
        assert!(valid_secret(&"s".repeat(32)));
    }

    // ── 희귀: 재시도 정책 ──
    #[test]
    fn retry_backoff_and_exhaustion() {
        let p = RetryPolicy::default();
        assert_eq!(p.decide(Delivery::Delivered, 1), QueueAction::Ack);
        assert_eq!(p.decide(Delivery::Delivered, 99), QueueAction::Ack);
        assert_eq!(
            p.decide(Delivery::Rejected(400), 1),
            QueueAction::Drop(DropReason::Rejected(400))
        );
        assert_eq!(
            p.decide(Delivery::Retry, 1),
            QueueAction::Retry { delay_secs: 30 }
        );
        assert_eq!(
            p.decide(Delivery::Retry, 2),
            QueueAction::Retry { delay_secs: 60 }
        );
        assert_eq!(
            p.decide(Delivery::Retry, 3),
            QueueAction::Retry { delay_secs: 120 }
        );
        assert_eq!(
            p.decide(Delivery::Retry, 8),
            QueueAction::Retry { delay_secs: 3600 }
        );
        assert_eq!(
            p.decide(Delivery::Retry, 9),
            QueueAction::Retry { delay_secs: 3600 }
        );
        assert_eq!(
            p.decide(Delivery::Retry, 10),
            QueueAction::Drop(DropReason::Exhausted)
        );
        assert_eq!(
            p.decide(Delivery::Retry, 500),
            QueueAction::Drop(DropReason::Exhausted)
        );
        // 0(비정상 값)과 큰 값에서도 넘치지 않는다
        assert_eq!(p.delay_after(0), 30);
        assert_eq!(p.delay_after(u32::MAX), 3600);
    }
}

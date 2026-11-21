//! 운영 경로: Cloudflare Worker가 서명해서 보낸 이벤트를 받는다.

use crate::AppState;
use analytics_core::Event;
use analytics_core::edge_auth::{self, HEADER_SIGNATURE, HEADER_TIMESTAMP};
use analytics_core::timeutil::now_ms;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Deserialize;
use std::sync::atomic::Ordering::Relaxed;

/// Worker → ingest 본문.
#[derive(Debug, Deserialize)]
pub struct EdgeForward {
    pub events: Vec<Event>,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/internal/collect", post(collect))
}

async fn collect(State(s): State<AppState>, headers: HeaderMap, body: Bytes) -> StatusCode {
    if s.cfg.edge_secret.is_empty() {
        return StatusCode::NOT_FOUND;
    }
    if !verify_collect(&s.cfg.edge_secret, &headers, &body, now_ms()) {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(fwd) = serde_json::from_slice::<EdgeForward>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let n = fwd.events.len() as u64;
    // 프로젝트 설정(client_purchase_as_order)은 주문 서비스가 확인한다.
    let purchases: Vec<Event> = fwd
        .events
        .iter()
        .filter(|e| e.event_name == analytics_core::orders::PURCHASE_EVENT)
        .cloned()
        .collect();
    match s.pipeline.try_send(fwd.events) {
        Ok(()) => {
            s.orders.spawn_client_purchases(&purchases);
            s.metrics.events_received.fetch_add(n, Relaxed);
            StatusCode::NO_CONTENT
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// `/internal/collect` 서명 검증 (v1, 내부 경로 고정, 전달 헤더 없음).
fn verify_collect(secret: &[u8], headers: &HeaderMap, body: &[u8], now_ms: i64) -> bool {
    let ts = headers
        .get(HEADER_TIMESTAMP)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());
    let sig = headers.get(HEADER_SIGNATURE).and_then(|v| v.to_str().ok());
    let (Some(ts), Some(sig)) = (ts, sig) else {
        return false;
    };
    edge_auth::verify(
        secret,
        &edge_auth::SignedRequest::collect(ts, body),
        sig,
        now_ms,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use analytics_core::edge_auth::SignedRequest;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef-edge";
    const NOW: i64 = 1_795_219_200_000;
    const BODY: &[u8] = br#"{"events":[]}"#;

    fn headers(ts: i64, sig: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(HEADER_TIMESTAMP, ts.to_string().parse().unwrap());
        h.insert(HEADER_SIGNATURE, sig.parse().unwrap());
        h
    }

    #[test]
    fn collect_accepts_v1_collect_signature() {
        let sig = edge_auth::sign(SECRET, &SignedRequest::collect(NOW, BODY));
        assert!(verify_collect(SECRET, &headers(NOW, &sig), BODY, NOW));
        assert!(!verify_collect(SECRET, &HeaderMap::new(), BODY, NOW));
    }

    #[test]
    fn collect_rejects_signature_made_for_orders() {
        // /internal/orders로 서명된 본문을 /internal/collect로 재생할 수 없다
        let orders = SignedRequest {
            internal_path: edge_auth::ORDERS_PATH,
            ..SignedRequest::collect(NOW, BODY)
        };
        let sig = edge_auth::sign(SECRET, &orders);
        assert!(!verify_collect(SECRET, &headers(NOW, &sig), BODY, NOW));
    }

    #[test]
    fn collect_extreme_timestamps_do_not_panic() {
        for ts in [i64::MIN, i64::MAX] {
            let sig = edge_auth::sign(SECRET, &SignedRequest::collect(ts, BODY));
            assert!(!verify_collect(SECRET, &headers(ts, &sig), BODY, NOW));
        }
    }
}

//! 운영 경로: Cloudflare Worker가 서명해서 보낸 이벤트를 받는다.

use crate::AppState;
use analytics_core::Event;
use analytics_core::edge_auth::{self, HEADER_SIGNATURE, HEADER_TIMESTAMP};
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
    let ts = headers
        .get(HEADER_TIMESTAMP)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());
    let sig = headers.get(HEADER_SIGNATURE).and_then(|v| v.to_str().ok());
    let (Some(ts), Some(sig)) = (ts, sig) else {
        return StatusCode::UNAUTHORIZED;
    };
    if !edge_auth::verify(&s.cfg.edge_secret, ts, &body, sig, now_ms()) {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(fwd) = serde_json::from_slice::<EdgeForward>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let n = fwd.events.len() as u64;
    match s.pipeline.try_send(fwd.events) {
        Ok(()) => {
            s.metrics.events_received.fetch_add(n, Relaxed);
            StatusCode::NO_CONTENT
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

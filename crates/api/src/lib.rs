//! api: 대시보드(Dioxus)와 관리·리포팅 API 서버.

pub mod config;

pub use config::{ApiConfig, LoginLimitConfig};

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pg: analytics_pg::PgPool,
    pub ch: analytics_ch::Client,
    pub config: Arc<ApiConfig>,
}

impl AppState {
    pub fn new(pg: analytics_pg::PgPool, ch: analytics_ch::Client, config: ApiConfig) -> Self {
        Self {
            pg,
            ch,
            config: Arc::new(config),
        }
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .with_state(state)
}

/// 의존 서비스 연결 확인.
async fn readyz(State(s): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    let pg = s.pg.acquire().await.is_ok();
    let ch = analytics_ch::ping(&s.ch).await.is_ok();
    let status = if pg && ch {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(json!({ "postgres": pg, "clickhouse": ch })))
}

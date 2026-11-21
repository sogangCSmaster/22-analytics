//! api: 대시보드(Dioxus)와 리포팅 API 서버.
//!
//! Phase 0에서는 헬스체크만 제공한다. 인증·프로젝트 관리·리포트는 Phase 1부터 추가한다.

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde_json::json;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    pg: analytics_pg::PgPool,
    ch: analytics_ch::Client,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let bind = std::env::var("API_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://analytics:analytics@localhost:5432/analytics".into());

    let state = AppState {
        pg: analytics_pg::connect(&database_url).await?,
        ch: analytics_ch::ChConfig::from_env().client(),
    };

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let listener = TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "api 시작");
    axum::serve(listener, app).await?;
    Ok(())
}

/// 의존 서비스 연결 확인.
async fn readyz(State(s): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    let pg = sqlx_ping(&s.pg).await;
    let ch = analytics_ch::ping(&s.ch).await.is_ok();
    let status = if pg && ch {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(json!({ "postgres": pg, "clickhouse": ch })))
}

async fn sqlx_ping(pool: &analytics_pg::PgPool) -> bool {
    pool.acquire().await.is_ok()
}

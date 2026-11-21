//! ingest: 이벤트 수신 서버.
//!
//! - 운영: Cloudflare Worker가 검증·보강한 이벤트를 `/internal/collect`로 보낸다 (HMAC 서명).
//! - 로컬 개발(`EDGE_EMULATION=true`): Worker 역할까지 직접 수행한다.
//!   `/t.js`, `/v1/config`, `/v1/collect`, `/dev/` 테스트 페이지를 제공한다.

mod archive;
mod config;
mod emulate;
mod internal;
mod metrics;
mod orders;
mod pipeline;

use axum::{Router, routing::get};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<config::Config>,
    pub pipeline: pipeline::Pipeline,
    pub metrics: Arc<metrics::Metrics>,
    pub emulator: Option<Arc<emulate::Emulator>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cfg = Arc::new(config::Config::from_env()?);
    let ch = analytics_ch::ChConfig::from_env();
    let client = ch.client();
    analytics_ch::ping(&client).await?;

    let metrics = Arc::new(metrics::Metrics::default());
    let (pipeline, writer) = pipeline::start(client, cfg.clone(), metrics.clone()).await?;

    let emulator = if cfg.edge_emulation {
        let pool = analytics_pg::connect(&cfg.database_url).await?;
        tracing::warn!("엣지 에뮬레이션 모드: 로컬 개발 전용");
        Some(Arc::new(emulate::Emulator::new(
            pool,
            cfg.tracker_dist.clone(),
        )))
    } else {
        None
    };

    let state = AppState {
        cfg: cfg.clone(),
        pipeline: pipeline.clone(),
        metrics,
        emulator: emulator.clone(),
    };

    let mut app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(metrics::handler))
        .merge(internal::router());
    if emulator.is_some() {
        app = app.merge(emulate::router(&cfg));
    }
    let app = app
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let listener = TcpListener::bind(&cfg.bind).await?;
    tracing::info!(bind = %cfg.bind, "ingest 시작");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // 남은 버퍼를 비우고 종료한다.
    drop(pipeline);
    writer.await??;
    tracing::info!("ingest 종료");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

//! api 서버 진입점. 라우터는 `lib.rs`의 [`analytics_api::build_router`]에 있다.

use analytics_api::{ApiConfig, AppState, build_router};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

/// 전역 할당자: 요청·JSON 직렬화의 작은 할당이 많다.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = ApiConfig::from_env();
    let pg = analytics_pg::connect(&config.database_url, config.pg_max_connections).await?;
    let ch = analytics_ch::ChConfig::from_env().guarded_client();
    let bind = config.bind.clone();
    let app = build_router(AppState::new(pg, ch, config))
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let listener = TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "api 시작");
    // 소켓 상대 주소를 [`analytics_api::extract::ClientIp`]가 읽는다.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

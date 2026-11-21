//! api: 대시보드(Dioxus)와 관리·리포팅 API 서버.
//!
//! ## 인증·권한 요약
//! - 인증: 세션 쿠키(`COOKIE_SECURE=true`면 `__Host-at_session`, 아니면 `at_session`).
//!   없거나 만료면 401. 슬라이딩 연장 시 쿠키도 다시 보낸다.
//! - 클라이언트 IP: 소켓 주소. `TRUST_PROXY_HEADERS=true`일 때만 프록시 헤더를 믿는다
//!   (서버는 `into_make_service_with_connect_info`로 띄워야 한다, `main.rs`).
//! - 전역: 슈퍼 관리자(대행사 관리자)는 모든 광고주·프로젝트에 `admin`으로 접근한다.
//! - 광고주 역할: `viewer`(읽기) < `analyst`(프로젝트 설정 편집) < `admin`(광고주·멤버·프로젝트 관리).
//! - 접근할 수 없는 광고주·프로젝트는 404, 역할이 부족하면 403.
//! - 변경 요청은 CSRF 검사([`security`]): 허용 Origin + JSON(또는 CSV) Content-Type.
//!
//! 리포트(`routes::reports`)는 모든 멤버 역할이 읽는다. ClickHouse 장애는 503 `storage_unavailable`.

pub mod clock;
pub mod config;
pub mod error;
pub mod extract;
pub mod ratelimit;
pub mod routes;
pub mod security;
pub mod validate;

pub use config::{ApiConfig, LoginLimitConfig};

use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router, extract::State, http::StatusCode};
use clock::{Clock, SystemClock};
use ratelimit::LoginLimiter;
use routes::{ad_spend, api_keys, auth, orgs, project_config, projects, reports, users};
use serde_json::json;
use std::sync::Arc;

/// JSON 본문 상한
pub const MAX_JSON_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub pg: analytics_pg::PgPool,
    pub ch: analytics_ch::Client,
    pub config: Arc<ApiConfig>,
    pub limiter: Arc<LoginLimiter>,
    /// 리포트 기준 시각. 테스트에서 [`AppState::with_clock`]으로 고정한다.
    pub clock: Arc<dyn Clock>,
}

impl AppState {
    pub fn new(pg: analytics_pg::PgPool, ch: analytics_ch::Client, config: ApiConfig) -> Self {
        let limiter = Arc::new(LoginLimiter::new(config.login_limit.clone()));
        Self {
            pg,
            ch,
            config: Arc::new(config),
            limiter,
            clock: Arc::new(SystemClock),
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }
}

pub fn build_router(state: AppState) -> Router {
    let v1 = Router::new()
        // 인증
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        // 사용자 (슈퍼 관리자)
        .route("/users", get(users::list).post(users::create))
        // 광고주
        .route("/orgs", get(orgs::list).post(orgs::create))
        .route("/orgs/{org_id}", get(orgs::get).patch(orgs::update))
        .route(
            "/orgs/{org_id}/members",
            get(orgs::members).post(orgs::add_member),
        )
        .route(
            "/orgs/{org_id}/members/{user_id}",
            delete(orgs::remove_member),
        )
        .route(
            "/orgs/{org_id}/projects",
            get(projects::list).post(projects::create),
        )
        // 프로젝트
        .route(
            "/projects/{project_id}",
            get(projects::get).patch(projects::update),
        )
        .route("/projects/{project_id}/snippet", get(projects::snippet))
        .route(
            "/projects/{project_id}/settings",
            get(project_config::get_settings).put(project_config::put_settings),
        )
        .route(
            "/projects/{project_id}/origins",
            get(project_config::list_origins).post(project_config::add_origin),
        )
        .route(
            "/projects/{project_id}/origins/{origin}",
            delete(project_config::remove_origin),
        )
        .route(
            "/projects/{project_id}/channel-rules",
            get(project_config::list_rules).post(project_config::create_rule),
        )
        .route(
            "/projects/{project_id}/channel-rules/{rule_id}",
            patch(project_config::update_rule).delete(project_config::delete_rule),
        )
        .route(
            "/projects/{project_id}/attribution-settings",
            get(project_config::get_attribution).put(project_config::put_attribution),
        )
        .route(
            "/projects/{project_id}/ad-spend",
            get(ad_spend::list)
                .post(ad_spend::upload)
                .layer(DefaultBodyLimit::max(ad_spend::MAX_BODY_BYTES)),
        )
        // S2S 비밀 키
        .route(
            "/projects/{project_id}/api-keys",
            get(api_keys::list).post(api_keys::create),
        )
        .route(
            "/projects/{project_id}/api-keys/{key_id}",
            delete(api_keys::revoke),
        )
        // 리포트
        .route(
            "/projects/{project_id}/reports/overview",
            get(reports::overview),
        )
        .route(
            "/projects/{project_id}/reports/timeseries",
            get(reports::timeseries),
        )
        .route(
            "/projects/{project_id}/reports/channels",
            get(reports::channels),
        )
        .route("/projects/{project_id}/reports/pages", get(reports::pages))
        .route(
            "/projects/{project_id}/reports/events",
            get(reports::events),
        )
        .route(
            "/projects/{project_id}/reports/events/{event_name}/properties/{prop_key}",
            get(reports::event_properties),
        )
        .route(
            "/projects/{project_id}/reports/realtime",
            get(reports::realtime),
        )
        .route(
            "/projects/{project_id}/reports/funnel",
            get(reports::funnel),
        )
        .route(
            "/projects/{project_id}/reports/revenue",
            get(reports::revenue),
        )
        .route(
            "/projects/{project_id}/reports/overlap",
            get(reports::overlap),
        )
        .route(
            "/projects/{project_id}/reports/orders",
            get(reports::orders),
        )
        .route(
            "/projects/{project_id}/reports/orders/{order_id}",
            get(reports::order_detail),
        );

    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .nest("/v1", v1)
        .fallback(|| async { error::ApiError::not_found() })
        .layer(DefaultBodyLimit::max(MAX_JSON_BYTES))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            extract::session_refresh,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::csrf_guard,
        ))
        .layer(security::cors_layer(&state.config.dashboard_origins))
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

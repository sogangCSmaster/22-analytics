//! 로컬 개발용 엣지 에뮬레이션: Cloudflare Worker가 하는 일을 ingest가 대신한다.
//! 검증·보강 로직은 Worker와 같은 `analytics_core::collect`를 쓴다.

use crate::AppState;
use crate::config::Config;
use crate::lock;
use analytics_core::cache::TtlCache;
use analytics_core::collect::cookie::{VISITOR_COOKIE, visitor_id};
use analytics_core::collect::{self, ProjectConfig, RequestCtx};
use analytics_core::event::{MAX_BATCH_BYTES, WireBatch};
use analytics_core::timeutil::now_ms;
use axum::{
    Router,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering::Relaxed;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::ServeDir;
use uuid::Uuid;

const CONFIG_TTL_MS: i64 = 60_000;
const CONFIG_CACHE_MAX: usize = 10_000;

pub struct Emulator {
    pool: PgPool,
    tracker_dist: PathBuf,
    /// 요청마다 설정(허용 도메인·설정 JSON)을 복제하지 않도록 `Arc`로 들고 있는다.
    cache: Mutex<TtlCache<String, Arc<ProjectConfig>>>,
}

impl Emulator {
    pub fn new(pool: PgPool, tracker_dist: PathBuf) -> Self {
        Self {
            pool,
            tracker_dist,
            cache: Mutex::new(TtlCache::new(CONFIG_TTL_MS, CONFIG_CACHE_MAX)),
        }
    }

    /// Worker의 KV 조회 + 메모리 캐시와 같은 역할.
    async fn project(&self, key: &str) -> anyhow::Result<Option<Arc<ProjectConfig>>> {
        if let Some(cfg) = lock(&self.cache).get(key, now_ms()) {
            return Ok(cfg);
        }
        let cfg = analytics_pg::projects::config_by_public_key(&self.pool, key)
            .await?
            .map(Arc::new);
        lock(&self.cache).put(key.to_owned(), cfg.clone(), now_ms());
        Ok(cfg)
    }
}

pub fn router(cfg: &Config) -> Router<AppState> {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE])
        .expose_headers([header::HeaderName::from_static("x-cfg-v")]);
    Router::new()
        .route("/t.js", get(tracker_js))
        .route("/v1/config", get(public_config))
        .route("/v1/collect", post(collect))
        .layer(cors)
        .nest_service("/dev", ServeDir::new(&cfg.dev_site_dir))
}

async fn tracker_js(State(s): State<AppState>) -> Response {
    let emu = s.emulator.as_ref().expect("에뮬레이션 모드에서만 등록된다");
    match tokio::fs::read(emu.tracker_dist.join("t.js")).await {
        Ok(js) => (
            [
                (
                    header::CONTENT_TYPE,
                    "application/javascript; charset=utf-8",
                ),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            js,
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            "tracker/dist/t.js 없음: `npm run build --prefix tracker` 실행",
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct ConfigQuery {
    k: String,
}

async fn public_config(State(s): State<AppState>, Query(q): Query<ConfigQuery>) -> Response {
    let emu = s.emulator.as_ref().expect("에뮬레이션 모드에서만 등록된다");
    match emu.project(&q.k).await {
        Ok(Some(p)) if p.active => (
            [(header::CACHE_CONTROL, "max-age=300")],
            axum::Json(p.settings.public_config(p.settings_version)),
        )
            .into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "프로젝트 설정 조회 실패");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

async fn collect(State(s): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let emu = s.emulator.as_ref().expect("에뮬레이션 모드에서만 등록된다");
    if body.len() > MAX_BATCH_BYTES {
        return status_with_reason(StatusCode::PAYLOAD_TOO_LARGE, "too_large");
    }
    let Ok(batch) = serde_json::from_slice::<WireBatch>(&body) else {
        return status_with_reason(StatusCode::BAD_REQUEST, "bad_json");
    };
    let project = match emu.project(&batch.k).await {
        Ok(Some(p)) => p,
        Ok(None) => return reject(&s, "unknown_key"),
        Err(e) => {
            tracing::error!(error = %e, "프로젝트 설정 조회 실패");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };

    let h = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let ctx = RequestCtx {
        origin: h("origin"),
        referer: h("referer"),
        user_agent: h("user-agent").unwrap_or(""),
        country: h("cf-ipcountry").unwrap_or("KR"),
        region: "",
        city: "",
        cookie_vid: cookie_vid(&headers),
        dnt: h("dnt") == Some("1"),
        now_ms: now_ms(),
    };

    let processed = match collect::process(&ctx, &project, batch, Uuid::new_v4) {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(reason = %e, "배치 거부");
            return reject(&s, e.code());
        }
    };

    let n = processed.events.len() as u64;
    s.metrics
        .events_dropped
        .fetch_add(processed.dropped as u64, Relaxed);
    // client_purchase_as_order: purchase 이벤트를 클라이언트 주문으로도 투영한다 (§5.3).
    let purchases = if project.settings.client_purchase_as_order {
        crate::orders::purchases_of(&processed.events)
    } else {
        vec![]
    };
    if s.pipeline.try_send(processed.events).is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    s.orders.spawn_client_purchases(purchases);
    s.metrics.events_received.fetch_add(n, Relaxed);

    let mut resp = StatusCode::NO_CONTENT.into_response();
    set_cfg_version(&mut resp, project.settings_version);
    if processed.issue_cookie {
        // 운영에서는 커스텀 호스트명의 상위 도메인으로 Domain을 지정하고 Secure를 붙인다.
        let c = format!(
            "{VISITOR_COOKIE}={}; Path=/; Max-Age=31536000; HttpOnly; SameSite=Lax",
            processed.visitor_id
        );
        if let Ok(v) = HeaderValue::from_str(&c) {
            resp.headers_mut().insert(header::SET_COOKIE, v);
        }
    }
    resp
}

/// 트래커가 설정 변경을 알 수 있게 설정 버전을 붙인다 (엣지 `cors::config_version_headers`와 같다).
/// expose 헤더는 CORS 레이어도 붙이지만, 응답만으로 계약이 드러나도록 여기서도 넣는다.
fn set_cfg_version(resp: &mut Response, settings_version: u32) {
    let h = resp.headers_mut();
    h.insert("x-cfg-v", HeaderValue::from(settings_version));
    h.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("x-cfg-v"),
    );
}

fn reject(s: &AppState, code: &'static str) -> Response {
    s.metrics.batches_rejected.fetch_add(1, Relaxed);
    // 운영 Worker는 거부 사유를 숨기고 204만 돌려준다. 로컬에서는 디버깅용으로 헤더에 남긴다.
    status_with_reason(StatusCode::NO_CONTENT, code)
}

fn status_with_reason(status: StatusCode, code: &'static str) -> Response {
    let mut r = status.into_response();
    r.headers_mut()
        .insert("x-at-status", HeaderValue::from_static(code));
    r
}

/// `Cookie` 헤더(여러 개일 수 있다)에서 엣지와 같은 규칙으로 `_at_vid`를 읽는다.
fn cookie_vid(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|raw| visitor_id(Some(raw)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_response_exposes_settings_version() {
        let mut r = StatusCode::NO_CONTENT.into_response();
        set_cfg_version(&mut r, 7);
        assert_eq!(r.headers()["x-cfg-v"], "7");
        assert_eq!(r.headers()["access-control-expose-headers"], "x-cfg-v");
    }
}

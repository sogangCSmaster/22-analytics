//! CORS와 CSRF 방어.
//!
//! 세션은 쿠키(SameSite=Lax)로 전달되므로, 변경 요청(POST/PUT/PATCH/DELETE)에는 두 가지를 요구한다.
//! 1. `Origin`이 `DASHBOARD_ORIGINS` 목록에 정확히 일치 (없으면 거부)
//! 2. `Content-Type`이 `application/json` (광고비 CSV는 `text/csv`) — HTML form이 보낼 수 없는
//!    타입이라 브라우저가 반드시 CORS preflight를 거친다.
//!
//! 위반 시 403 `csrf_rejected`. 모든 응답에 `nosniff`를 붙이고, 핸들러가 `Cache-Control`을
//! 정하지 않은 응답에는 `no-store`를 붙인다 (닫힌 기간 리포트만 `private, max-age=60`).

use crate::AppState;
use crate::error::ApiError;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::time::Duration;
use tower_http::cors::{AllowOrigin, CorsLayer};

/// JSON 이외에 허용하는 변경 요청 Content-Type (둘 다 CORS safelist 밖이라 preflight 필요).
const ALLOWED_CONTENT_TYPES: &[&str] = &["application/json", "text/csv"];

fn is_mutating(m: &Method) -> bool {
    matches!(
        *m,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// `application/json; charset=utf-8` → `application/json`
fn mime_essence(v: &str) -> String {
    v.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

pub async fn csrf_guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if is_mutating(req.method()) {
        let h = req.headers();
        let origin_ok = h
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|o| state.config.dashboard_origins.iter().any(|a| a == o));
        let ct_ok = h
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(mime_essence)
            .is_some_and(|ct| ALLOWED_CONTENT_TYPES.contains(&ct.as_str()));
        if !origin_ok || !ct_ok {
            tracing::warn!(
                method = %req.method(),
                path = %req.uri().path(),
                origin_ok,
                ct_ok,
                "CSRF 검사 실패"
            );
            return with_security_headers(ApiError::csrf().into_response());
        }
    }
    with_security_headers(next.run(req).await)
}

fn with_security_headers(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}

/// 대시보드 origin만 허용하고 자격 증명(쿠키)을 허용하는 CORS.
pub fn cors_layer(origins: &[String]) -> CorsLayer {
    let list: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(list))
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([header::CONTENT_TYPE])
        .max_age(Duration::from_secs(600))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_essence_strips_params() {
        assert_eq!(
            mime_essence("Application/JSON; charset=utf-8"),
            "application/json"
        );
        assert_eq!(mime_essence("text/csv"), "text/csv");
        assert_eq!(mime_essence(""), "");
    }
}

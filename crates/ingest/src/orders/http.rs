//! 주문 API HTTP 라우트.
//!
//! - 로컬(`EDGE_EMULATION=true`): `POST /v1/orders`, `POST /v1/orders:batch` (`Authorization: Bearer sk_...`)
//! - 운영: `POST /internal/orders` (Worker가 전달). 계약:
//!   - `x-edge-timestamp`, `x-edge-signature`: v1 서명 ([`edge_auth`]). 내부 경로·본문 해시와
//!     아래 전달 헤더 3개(없으면 빈 문자열)를 모두 덮는다
//!   - `x-forwarded-authorization`: 클라이언트의 `Authorization` 헤더 원문 (원점이 다시 검증)
//!   - `x-forwarded-path`: `/v1/orders` 또는 `/v1/orders:batch`
//!   - `idempotency-key`: 클라이언트의 `Idempotency-Key` (있을 때만)
//!   - 본문은 클라이언트 원문. 응답 상태·본문은 Worker가 그대로 클라이언트에 돌려준다.
//!
//! 응답: 단건 `202 {"accepted":true,"warnings":[...]}`, 일괄 `202 {"results":[...]}`,
//! 오류 `{"error":"code","message":"..."}` (400/401/403/404/409/413/422/503).
//! 409 `idempotency_key_reused`: 같은 (이벤트 유형, 멱등 키)로 금액·시각이 다른 요청이 이미 있다.

use super::{OrderService, ProjectCtx};
use analytics_core::edge_auth::{self, HEADER_SIGNATURE, HEADER_TIMESTAMP};
use analytics_core::orders::{
    self as core_orders, MAX_BATCH_BODY_BYTES, MAX_BATCH_ORDERS, MAX_SINGLE_BODY_BYTES,
    OrderProject,
};
use analytics_core::timeutil::now_ms;
use analytics_pg::api_keys::{self, SCOPE_ORDERS_WRITE};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Semaphore;

pub const PATH_SINGLE: &str = "/v1/orders";
pub const PATH_BATCH: &str = "/v1/orders:batch";
pub const HEADER_FORWARDED_AUTH: &str = "x-forwarded-authorization";
pub const HEADER_FORWARDED_PATH: &str = "x-forwarded-path";
pub const HEADER_IDEMPOTENCY_KEY: &str = "idempotency-key";
/// 일괄 요청 안에서 동시에 처리하는 주문 수.
const BATCH_CONCURRENCY: usize = 16;

#[derive(Clone)]
pub struct OrdersHttp {
    pub svc: Arc<OrderService>,
    /// 비어 있으면 `/internal/orders`를 등록하지 않는다.
    pub edge_secret: Arc<Vec<u8>>,
    /// 참이면 `/v1/orders*`를 직접 받는다 (로컬 엣지 에뮬레이션).
    pub emulation: bool,
}

pub fn router<S: Clone + Send + Sync + 'static>(st: OrdersHttp) -> Router<S> {
    let mut r = Router::new();
    if st.emulation {
        r = r
            .route(PATH_SINGLE, post(emulated_single))
            .route(PATH_BATCH, post(emulated_batch));
    }
    if !st.edge_secret.is_empty() {
        r = r.route("/internal/orders", post(internal));
    }
    r.with_state(st)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Single,
    Batch,
}

fn header<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

async fn emulated_single(
    State(st): State<OrdersHttp>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let auth = header(&headers, "authorization");
    let idem = header(&headers, HEADER_IDEMPOTENCY_KEY);
    handle(&st, Kind::Single, auth, idem, &body).await
}

async fn emulated_batch(State(st): State<OrdersHttp>, headers: HeaderMap, body: Bytes) -> Response {
    let auth = header(&headers, "authorization");
    handle(&st, Kind::Batch, auth, None, &body).await
}

async fn internal(State(st): State<OrdersHttp>, headers: HeaderMap, body: Bytes) -> Response {
    let ts = header(&headers, HEADER_TIMESTAMP).and_then(|v| v.parse::<i64>().ok());
    let sig = header(&headers, HEADER_SIGNATURE);
    let (Some(ts), Some(sig)) = (ts, sig) else {
        return error(
            StatusCode::UNAUTHORIZED,
            "invalid_signature",
            "엣지 서명이 없습니다",
        );
    };
    let signed = edge_auth::SignedRequest {
        internal_path: edge_auth::ORDERS_PATH,
        timestamp_ms: ts,
        body: &body,
        forwarded_path: header(&headers, HEADER_FORWARDED_PATH).unwrap_or(""),
        forwarded_authorization: header(&headers, HEADER_FORWARDED_AUTH).unwrap_or(""),
        idempotency_key: header(&headers, HEADER_IDEMPOTENCY_KEY).unwrap_or(""),
    };
    if !edge_auth::verify(&st.edge_secret, &signed, sig, now_ms()) {
        return error(
            StatusCode::UNAUTHORIZED,
            "invalid_signature",
            "엣지 서명이 올바르지 않습니다",
        );
    }
    let kind = match header(&headers, HEADER_FORWARDED_PATH) {
        Some(PATH_SINGLE) => Kind::Single,
        Some(PATH_BATCH) => Kind::Batch,
        _ => return error(StatusCode::NOT_FOUND, "not_found", "알 수 없는 경로"),
    };
    let auth = header(&headers, HEADER_FORWARDED_AUTH);
    let idem = match kind {
        Kind::Single => header(&headers, HEADER_IDEMPOTENCY_KEY),
        Kind::Batch => None,
    };
    handle(&st, kind, auth, idem, &body).await
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

const KEY_REUSED: &str = "idempotency_key_reused";
const KEY_REUSED_MESSAGE: &str = "같은 멱등 키로 내용이 다른 요청이 이미 처리되었습니다";

fn key_reused() -> Response {
    error(StatusCode::CONFLICT, KEY_REUSED, KEY_REUSED_MESSAGE)
}

fn storage_unavailable(e: &anyhow::Error) -> Response {
    tracing::error!(error = %e, "주문 저장소 오류");
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "storage_unavailable",
        "일시적인 저장소 오류입니다. 같은 요청을 다시 보내 주세요",
    )
}

/// `Bearer <token>` (스킴 대소문자 무시).
fn bearer(v: &str) -> Option<&str> {
    let (scheme, token) = v.trim().split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

async fn handle(
    st: &OrdersHttp,
    kind: Kind,
    auth: Option<&str>,
    idempotency_key: Option<&str>,
    body: &[u8],
) -> Response {
    let limit = match kind {
        Kind::Single => MAX_SINGLE_BODY_BYTES,
        Kind::Batch => MAX_BATCH_BODY_BYTES,
    };
    if body.len() > limit {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "본문이 너무 큽니다",
        );
    }
    let project = match authenticate(st, auth).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return error(StatusCode::BAD_REQUEST, "bad_json", "JSON 형식이 아닙니다");
    };
    match kind {
        Kind::Single => single(st, &project, value, idempotency_key).await,
        Kind::Batch => batch(st, &project, value).await,
    }
}

async fn authenticate(st: &OrdersHttp, auth: Option<&str>) -> Result<ProjectCtx, Response> {
    let Some(token) = auth.and_then(bearer) else {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "missing_key",
            "Authorization: Bearer sk_... 헤더가 필요합니다",
        ));
    };
    let key = match api_keys::verify(st.svc.pg(), token).await {
        Ok(Some(k)) => k,
        Ok(None) => {
            return Err(error(
                StatusCode::UNAUTHORIZED,
                "invalid_key",
                "유효하지 않은 비밀 키입니다",
            ));
        }
        Err(e) => return Err(storage_unavailable(&e)),
    };
    if !key.has_scope(SCOPE_ORDERS_WRITE) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            "orders:write 권한이 없는 키입니다",
        ));
    }
    match st.svc.project(key.project_id).await {
        Ok(Some(p)) if p.active => Ok(p),
        Ok(_) => Err(error(
            StatusCode::FORBIDDEN,
            "project_inactive",
            "비활성 프로젝트입니다",
        )),
        Err(e) => Err(storage_unavailable(&e)),
    }
}

fn order_project(p: &ProjectCtx) -> OrderProject<'_> {
    OrderProject {
        project_id: p.project_id,
        currency: &p.currency,
        salt_hex: &p.salt_hex,
    }
}

async fn single(
    st: &OrdersHttp,
    project: &ProjectCtx,
    value: Value,
    idempotency_key: Option<&str>,
) -> Response {
    let v = match core_orders::validate(&order_project(project), &value, idempotency_key, now_ms())
    {
        Ok(v) => v,
        Err(e) => return error(StatusCode::UNPROCESSABLE_ENTITY, e.code(), &e.to_string()),
    };
    match st.svc.apply(v.event).await {
        Ok(a) if a.conflict => key_reused(),
        Ok(_) => (
            StatusCode::ACCEPTED,
            Json(json!({ "accepted": true, "warnings": v.warnings })),
        )
            .into_response(),
        Err(e) => storage_unavailable(&e),
    }
}

async fn batch(st: &OrdersHttp, project: &ProjectCtx, value: Value) -> Response {
    let items = match value {
        Value::Array(a) => a,
        Value::Object(mut o) => match o.remove("orders") {
            Some(Value::Array(a)) => a,
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "invalid_payload",
                    "{\"orders\": [...]} 형식이어야 합니다",
                );
            }
        },
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                "{\"orders\": [...]} 형식이어야 합니다",
            );
        }
    };
    if items.is_empty() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "empty_batch",
            "주문이 없습니다",
        );
    }
    if items.len() > MAX_BATCH_ORDERS {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_many_orders",
            "일괄 전송은 최대 500건입니다",
        );
    }

    let now = now_ms();
    let sem = Arc::new(Semaphore::new(BATCH_CONCURRENCY));
    let mut set = tokio::task::JoinSet::new();
    let mut results: Vec<Value> = vec![Value::Null; items.len()];
    let order_ids: Vec<Value> = items
        .iter()
        .map(|item| item.get("order_id").cloned().unwrap_or(Value::Null))
        .collect();
    for (i, item) in items.into_iter().enumerate() {
        let order_id = order_ids[i].clone();
        let validated = core_orders::validate(&order_project(project), &item, None, now);
        let v = match validated {
            Ok(v) => v,
            Err(e) => {
                results[i] = json!({ "order_id": order_id, "accepted": false,
                    "error": e.code(), "message": e.to_string() });
                continue;
            }
        };
        let svc = st.svc.clone();
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            let r = svc.apply(v.event).await;
            (i, order_id, v.warnings, r)
        });
    }
    let mut storage_failed = false;
    while let Some(joined) = set.join_next().await {
        let (i, order_id, warnings, r) = match joined {
            Ok(x) => x,
            Err(e) => {
                tracing::error!(error = %e, "일괄 주문 작업 실패");
                storage_failed = true;
                continue;
            }
        };
        results[i] = match r {
            Ok(a) if a.conflict => json!({ "order_id": order_id, "accepted": false,
                "error": KEY_REUSED, "message": KEY_REUSED_MESSAGE }),
            Ok(_) => json!({ "order_id": order_id, "accepted": true, "warnings": warnings }),
            Err(e) => {
                tracing::error!(error = %e, "주문 저장 실패");
                storage_failed = true;
                json!({ "order_id": order_id, "accepted": false, "error": "storage_unavailable",
                    "message": "일시적인 저장소 오류입니다. 같은 요청을 다시 보내 주세요" })
            }
        };
    }
    // 작업이 패닉한 항목(JoinError)은 결과가 비어 있다: order_id를 담은 internal 오류로 채운다.
    if fill_missing_results(&mut results, &order_ids) {
        storage_failed = true;
    }
    // 일부라도 저장에 실패하면 5xx: 고객사는 같은 배치를 다시 보내면 된다 (멱등).
    let status = if storage_failed {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::ACCEPTED
    };
    (status, Json(json!({ "results": results }))).into_response()
}

const INTERNAL_MESSAGE: &str = "내부 오류입니다. 같은 요청을 다시 보내 주세요";

/// 결과가 비어 있는(`null`) 항목을 `{order_id, accepted:false, error:"internal"}`로 채운다.
/// 하나라도 채웠으면 `true`.
fn fill_missing_results(results: &mut [Value], order_ids: &[Value]) -> bool {
    let mut filled = false;
    for (r, order_id) in results.iter_mut().zip(order_ids) {
        if r.is_null() {
            *r = json!({ "order_id": order_id, "accepted": false, "error": "internal",
                "message": INTERNAL_MESSAGE });
            filled = true;
        }
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panicked_batch_items_get_internal_error_not_null() {
        // 작업이 패닉(JoinError)해 결과를 못 받은 항목은 null이 아니라 order_id를 담은 오류로 채운다
        let order_ids = vec![json!("A"), json!("B"), Value::Null];
        let mut results = vec![
            json!({"order_id": "A", "accepted": true}),
            Value::Null,
            Value::Null,
        ];
        assert!(fill_missing_results(&mut results, &order_ids));
        assert_eq!(results[0]["accepted"], true);
        assert_eq!(
            results[1],
            json!({"order_id": "B", "accepted": false, "error": "internal",
                "message": INTERNAL_MESSAGE})
        );
        assert_eq!(results[2]["order_id"], Value::Null);
        assert_eq!(results[2]["error"], "internal");
        let mut full = vec![json!({"accepted": true})];
        assert!(!fill_missing_results(&mut full, &[json!("A")]));
    }
}

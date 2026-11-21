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
use analytics_core::orders::api::{ApiError, OrdersKind as Kind, bearer};
use analytics_core::orders::{self as core_orders, MAX_BATCH_ORDERS, OrderProject};
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

pub use analytics_core::orders::api::{
    HEADER_FORWARDED_AUTHORIZATION, HEADER_FORWARDED_PATH, HEADER_IDEMPOTENCY_KEY,
};
pub const PATH_SINGLE: &str = Kind::Single.path();
pub const PATH_BATCH: &str = Kind::Batch.path();
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
    let forwarded = edge_auth::Forwarded {
        path: header(&headers, HEADER_FORWARDED_PATH).unwrap_or(""),
        authorization: header(&headers, HEADER_FORWARDED_AUTHORIZATION).unwrap_or(""),
        idempotency_key: header(&headers, HEADER_IDEMPOTENCY_KEY).unwrap_or(""),
    };
    if !edge_auth::verify_headers(
        &st.edge_secret,
        edge_auth::ORDERS_PATH,
        header(&headers, HEADER_TIMESTAMP),
        header(&headers, HEADER_SIGNATURE),
        &body,
        forwarded,
        now_ms(),
    ) {
        return api_error(ApiError::InvalidSignature);
    }
    let Some(kind) = header(&headers, HEADER_FORWARDED_PATH).and_then(Kind::from_path) else {
        return api_error(ApiError::NotFound);
    };
    let auth = header(&headers, HEADER_FORWARDED_AUTHORIZATION);
    let idem = match kind {
        Kind::Single => header(&headers, HEADER_IDEMPOTENCY_KEY),
        Kind::Batch => None,
    };
    handle(&st, kind, auth, idem, &body).await
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

fn api_error(e: ApiError) -> Response {
    let status = StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(e.body())).into_response()
}

/// 일괄 결과 항목의 실패 표현.
fn item_error(order_id: Value, e: ApiError) -> Value {
    json!({ "order_id": order_id, "accepted": false, "error": e.code(), "message": e.message() })
}

fn storage_unavailable(e: &anyhow::Error) -> Response {
    tracing::error!(error = %e, "주문 저장소 오류");
    api_error(ApiError::StorageUnavailable)
}

async fn handle(
    st: &OrdersHttp,
    kind: Kind,
    auth: Option<&str>,
    idempotency_key: Option<&str>,
    body: &[u8],
) -> Response {
    if body.len() > kind.body_limit() {
        return api_error(ApiError::TooLarge);
    }
    let project = match authenticate(st, auth).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return api_error(ApiError::BadJson);
    };
    match kind {
        Kind::Single => single(st, &project, value, idempotency_key).await,
        Kind::Batch => batch(st, &project, value).await,
    }
}

async fn authenticate(st: &OrdersHttp, auth: Option<&str>) -> Result<ProjectCtx, Response> {
    let Some(token) = auth.and_then(bearer) else {
        return Err(api_error(ApiError::MissingKey));
    };
    let key = match api_keys::verify(st.svc.pg(), token).await {
        Ok(Some(k)) => k,
        Ok(None) => return Err(api_error(ApiError::InvalidKey)),
        Err(e) => return Err(storage_unavailable(&e)),
    };
    if !key.has_scope(SCOPE_ORDERS_WRITE) {
        return Err(api_error(ApiError::InsufficientScope));
    }
    match st.svc.project(key.project_id).await {
        Ok(Some(p)) if p.active => Ok(p),
        Ok(_) => Err(api_error(ApiError::ProjectInactive)),
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
        Ok(a) if a.conflict => api_error(ApiError::IdempotencyKeyReused),
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
            _ => return api_error(ApiError::InvalidPayload),
        },
        _ => return api_error(ApiError::InvalidPayload),
    };
    if items.is_empty() {
        return api_error(ApiError::EmptyBatch);
    }
    if items.len() > MAX_BATCH_ORDERS {
        return api_error(ApiError::TooManyOrders);
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
            Ok(a) if a.conflict => item_error(order_id, ApiError::IdempotencyKeyReused),
            Ok(_) => json!({ "order_id": order_id, "accepted": true, "warnings": warnings }),
            Err(e) => {
                tracing::error!(error = %e, "주문 저장 실패");
                storage_failed = true;
                item_error(order_id, ApiError::StorageUnavailable)
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

/// 결과가 비어 있는(`null`) 항목을 `{order_id, accepted:false, error:"internal"}`로 채운다.
/// 하나라도 채웠으면 `true`.
fn fill_missing_results(results: &mut [Value], order_ids: &[Value]) -> bool {
    let mut filled = false;
    for (r, order_id) in results.iter_mut().zip(order_ids) {
        if r.is_null() {
            *r = item_error(order_id.clone(), ApiError::Internal);
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
                "message": ApiError::Internal.message()})
        );
        assert_eq!(results[2]["order_id"], Value::Null);
        assert_eq!(results[2]["error"], "internal");
        let mut full = vec![json!({"accepted": true})];
        assert!(!fill_missing_results(&mut full, &[json!("A")]));
    }
}

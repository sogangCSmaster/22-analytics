//! S2S 주문 API 통합 테스트 (격리 PG·CH, 실제 라우터).

use super::http::{OrdersHttp, router};
use super::{OrderService, ProjectCtx};
use analytics_ch::ProjectScope;
use analytics_ch::orders::{self as ch_orders, OrderRow};
use analytics_core::collect::rehash_member;
use analytics_core::edge_auth;
use analytics_core::orders::{MAX_AMOUNT, MAX_BATCH_ORDERS, OrderStatus};
use analytics_testkit::TestDb;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef-edge";
const MEMBER: &str = "8d969eef6ecad3c29a3a629280e686cf0c3f5d5a86aff3ca12020c923adc6c92";
const VID: &str = "5c8a0d27-0861-4c76-b062-bc938f8c6833";

struct H {
    db: TestDb,
    app: Router,
    svc: Arc<OrderService>,
    key: String,
    project: i32,
    salt: String,
}

async fn make_project(db: &TestDb, settings: Value) -> (i32, String) {
    let org: i64 =
        sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('광고주') RETURNING id")
            .fetch_one(&db.pg)
            .await
            .unwrap();
    let salt = analytics_pg::new_salt_hex();
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO projects (org_id, name, public_key, salt, settings) VALUES ($1, 'p', $2, $3, $4) RETURNING id",
    )
    .bind(org)
    .bind(analytics_pg::new_public_key())
    .bind(&salt)
    .bind(settings)
    .fetch_one(&db.pg)
    .await
    .unwrap();
    (id, salt)
}

fn app_for(svc: Arc<OrderService>) -> Router {
    router(OrdersHttp {
        svc,
        edge_secret: Arc::new(SECRET.to_vec()),
        emulation: true,
    })
}

async fn setup() -> H {
    setup_with(json!({})).await
}

async fn setup_with(settings: Value) -> H {
    let db = TestDb::new().await;
    let (project, salt) = make_project(&db, settings).await;
    let (key, _) = analytics_pg::api_keys::create(&db.pg, project, None)
        .await
        .unwrap();
    let svc = Arc::new(OrderService::new(db.pg.clone(), db.ch.clone()));
    H {
        app: app_for(svc.clone()),
        db,
        svc,
        key,
        project,
        salt,
    }
}

async fn send(
    app: &Router,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let mut req = Request::post(path).header("content-type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

impl H {
    async fn post(&self, body: &Value) -> (StatusCode, Value) {
        self.post_raw("/v1/orders", body.to_string().into_bytes())
            .await
    }
    async fn batch(&self, body: &Value) -> (StatusCode, Value) {
        self.post_raw("/v1/orders:batch", body.to_string().into_bytes())
            .await
    }
    async fn post_raw(&self, path: &str, body: Vec<u8>) -> (StatusCode, Value) {
        let auth = format!("Bearer {}", self.key);
        send(&self.app, path, &[("authorization", &auth)], body).await
    }
    fn scope(&self) -> ProjectScope {
        ProjectScope::authorized(self.project as u32)
    }
    async fn order(&self, id: &str) -> Option<OrderRow> {
        ch_orders::current_order(&self.db.ch, self.scope(), id)
            .await
            .unwrap()
    }
    async fn event_count(&self, id: &str) -> usize {
        ch_orders::order_events(&self.db.ch, self.scope(), id)
            .await
            .unwrap()
            .len()
    }
    /// FINAL 없이 본 orders 행 수 (투영을 몇 번 썼는지).
    async fn projection_writes(&self, id: &str) -> u64 {
        self.db
            .ch
            .query("SELECT count() FROM orders WHERE project_id = ? AND order_id = ?")
            .bind(self.project as u32)
            .bind(id)
            .fetch_one()
            .await
            .unwrap()
    }
}

fn confirm(id: &str, net: i64) -> Value {
    json!({
        "event_type": "PURCHASE_CONFIRMED", "order_id": id, "visitor_id": VID,
        "member_id_hash": MEMBER, "currency": "KRW", "gross_amount": net, "net_amount": net,
        "ordered_at": "2026-11-20T18:45:12+09:00", "items": [{"id": "SKU1", "price": net, "qty": 1}]
    })
}

fn refund(id: &str, amount: i64, at: &str) -> Value {
    json!({ "event_type": "ORDER_REFUNDED", "order_id": id, "refund_amount": amount, "refunded_at": at })
}

fn cancel(id: &str, at: &str) -> Value {
    json!({ "event_type": "ORDER_CANCELLED", "order_id": id, "refund_amount": 100000, "cancelled_at": at })
}

fn err_code(v: &Value) -> &str {
    v["error"].as_str().unwrap_or("")
}

// ── 해피 케이스 ──────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn single_confirm_projects_order() {
    let h = setup().await;
    let (s, v) = h.post(&confirm("ORD-20261120-009901", 100_000)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(v, json!({"accepted": true, "warnings": []}));
    let o = h.order("ORD-20261120-009901").await.unwrap();
    assert_eq!(o.status, "CONFIRMED");
    assert_eq!(o.source, "s2s");
    assert_eq!(o.net_amount, 100_000);
    assert_eq!(o.effective_amount, 100_000);
    assert_eq!(o.visitor_id.unwrap().to_string(), VID);
    assert_eq!(o.member_hash, rehash_member(&h.salt, MEMBER));
    assert!(o.version > 1_700_000_000_000, "version은 unix ms 근사");
}

#[tokio::test(flavor = "multi_thread")]
async fn confirm_refund_cancel_lifecycle() {
    let h = setup().await;
    h.post(&confirm("O1", 100_000)).await;
    let v1 = h.order("O1").await.unwrap().version;
    let (s, _) = h.post(&refund("O1", 30_000, "2026-11-20T12:00:00Z")).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let o = h.order("O1").await.unwrap();
    assert_eq!(o.status, "PARTIALLY_REFUNDED");
    assert_eq!(o.effective_amount, 70_000);
    assert!(o.version > v1);
    h.post(&cancel("O1", "2026-11-21T09:12:04+09:00")).await;
    let o2 = h.order("O1").await.unwrap();
    assert_eq!(o2.status, "CANCELLED");
    assert_eq!(o2.effective_amount, 0);
    assert!(o2.version > o.version);
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_returns_per_item_results() {
    let h = setup().await;
    let body = json!({ "orders": [
        confirm("B1", 1000),
        { "event_type": "NOPE", "order_id": "B2" },
        with(confirm("B3", 3000), "visitor_id", json!("garbage")),
    ]});
    let (s, v) = h.batch(&body).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let r = v["results"].as_array().unwrap();
    assert_eq!(r.len(), 3);
    assert_eq!(r[0]["order_id"], "B1");
    assert_eq!(r[0]["accepted"], true);
    assert_eq!(r[1]["accepted"], false);
    assert_eq!(r[1]["error"], "unknown_event_type");
    assert_eq!(r[2]["accepted"], true);
    assert_eq!(r[2]["warnings"], json!(["invalid_visitor_id"]));
    assert!(h.order("B1").await.is_some());
    assert!(h.order("B2").await.is_none());
    assert_eq!(h.order("B3").await.unwrap().visitor_id, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_accepts_plain_array() {
    let h = setup().await;
    let (s, v) = h.batch(&json!([confirm("A1", 1), confirm("A2", 2)])).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(v["results"].as_array().unwrap().len(), 2);
}

fn with(mut v: Value, k: &str, val: Value) -> Value {
    v[k] = val;
    v
}

// ── 멱등성 ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn same_request_twice_is_noop() {
    let h = setup().await;
    let a = h.post(&confirm("O1", 100_000)).await;
    let b = h.post(&confirm("O1", 100_000)).await;
    assert_eq!(a, b);
    assert_eq!(h.event_count("O1").await, 1, "이벤트는 한 번만 append");
    assert_eq!(
        h.projection_writes("O1").await,
        1,
        "상태가 그대로면 투영을 다시 쓰지 않는다"
    );
    assert_eq!(h.order("O1").await.unwrap().effective_amount, 100_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn idempotency_key_header_first_wins() {
    let h = setup().await;
    let auth = format!("Bearer {}", h.key);
    let hdr = [
        ("authorization", auth.as_str()),
        ("idempotency-key", "evt-1"),
    ];
    let (s, _) = send(
        &h.app,
        "/v1/orders",
        &hdr,
        refund("O1", 10_000, "2026-11-20T12:00:00Z")
            .to_string()
            .into_bytes(),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    // 같은 키로 다른 금액을 재전송해도 처음 것만 반영된다
    let (s, _) = send(
        &h.app,
        "/v1/orders",
        &hdr,
        refund("O1", 99_000, "2026-11-20T13:00:00Z")
            .to_string()
            .into_bytes(),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let o = h.order("O1").await.unwrap();
    assert_eq!(o.refunded_amount, 10_000);
    assert_eq!(h.event_count("O1").await, 1);
    // 잘못된 키는 거부
    let bad = [("authorization", auth.as_str()), ("idempotency-key", "a b")];
    let (s, v) = send(
        &h.app,
        "/v1/orders",
        &bad,
        confirm("O2", 1).to_string().into_bytes(),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err_code(&v), "invalid_idempotency_key");
}

#[tokio::test(flavor = "multi_thread")]
async fn webhook_duplicate_refunds_counted_once() {
    let h = setup().await;
    h.post(&confirm("O1", 100_000)).await;
    let r = refund("O1", 30_000, "2026-11-20T12:00:00Z");
    for _ in 0..3 {
        assert_eq!(h.post(&r).await.0, StatusCode::ACCEPTED);
    }
    let (s, _) = h.batch(&json!({"orders": [r.clone(), r.clone()]})).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let o = h.order("O1").await.unwrap();
    assert_eq!(o.refunded_amount, 30_000);
    assert_eq!(o.effective_amount, 70_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_retry_is_idempotent() {
    let h = setup().await;
    let body = json!({"orders": [confirm("O1", 5000), refund("O1", 1000, "2026-11-20T12:00:00Z"), confirm("O2", 7000)]});
    let a = h.batch(&body).await;
    let o1 = h.order("O1").await.unwrap();
    let b = h.batch(&body).await;
    assert_eq!(a, b);
    assert_eq!(
        h.order("O1").await.unwrap(),
        o1,
        "재시도 후 상태·version 그대로"
    );
}

// ── 경계값·엣지 ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn amount_and_order_id_boundaries() {
    let h = setup().await;
    assert_eq!(h.post(&confirm("ZERO", 0)).await.0, StatusCode::ACCEPTED);
    assert_eq!(h.order("ZERO").await.unwrap().effective_amount, 0);
    assert_eq!(
        h.post(&confirm("MAX", MAX_AMOUNT)).await.0,
        StatusCode::ACCEPTED
    );
    assert_eq!(h.order("MAX").await.unwrap().net_amount, MAX_AMOUNT);
    let (s, v) = h.post(&confirm("OVER", MAX_AMOUNT + 1)).await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::UNPROCESSABLE_ENTITY, "amount_out_of_range")
    );
    let (s, v) = h.post(&confirm("NEG", -1)).await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::UNPROCESSABLE_ENTITY, "amount_out_of_range")
    );

    let id128 = "주".repeat(128);
    assert_eq!(h.post(&confirm(&id128, 1)).await.0, StatusCode::ACCEPTED);
    assert_eq!(h.order(&id128).await.unwrap().order_id, id128);
    let (s, v) = h.post(&confirm(&"a".repeat(129), 1)).await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_order_id")
    );
    let (s, v) = h.post(&confirm("", 1)).await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_order_id")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_and_oversized_bodies() {
    let h = setup().await;
    let (s, v) = h.post_raw("/v1/orders", vec![]).await;
    assert_eq!((s, err_code(&v)), (StatusCode::BAD_REQUEST, "bad_json"));
    let (s, v) = h.batch(&json!({"orders": []})).await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::UNPROCESSABLE_ENTITY, "empty_batch")
    );
    // 단건 64KB 초과
    let mut big = confirm("BIG", 1);
    big["pad"] = json!("x".repeat(64 * 1024));
    let (s, v) = h.post(&big).await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::PAYLOAD_TOO_LARGE, "too_large")
    );
    // 일괄 1MB 초과
    let mut huge = confirm("HUGE", 1);
    huge["pad"] = json!("x".repeat(1024 * 1024));
    let (s, v) = h.batch(&json!({"orders": [huge]})).await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::PAYLOAD_TOO_LARGE, "too_large")
    );
    // 501건
    let many: Vec<Value> = (0..=MAX_BATCH_ORDERS)
        .map(|i| confirm(&format!("M{i}"), 1))
        .collect();
    let (s, v) = h.batch(&json!({"orders": many})).await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::PAYLOAD_TOO_LARGE, "too_many_orders")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn max_batch_of_500_is_accepted() {
    let h = setup().await;
    let many: Vec<Value> = (0..MAX_BATCH_ORDERS)
        .map(|i| confirm(&format!("M{i}"), i as i64))
        .collect();
    let (s, v) = h.batch(&json!({"orders": many})).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), MAX_BATCH_ORDERS);
    assert!(results.iter().all(|r| r["accepted"] == true));
    assert_eq!(results[499]["order_id"], "M499", "결과는 입력 순서대로");
    let n: u64 =
        h.db.ch
            .query("SELECT count() FROM orders FINAL")
            .fetch_one()
            .await
            .unwrap();
    assert_eq!(n, 500);
}

// ── 희귀 케이스 ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn cancel_arriving_before_confirm() {
    let h = setup().await;
    h.post(&cancel("O1", "2026-11-21T09:12:04+09:00")).await;
    assert_eq!(h.order("O1").await.unwrap().status, "CANCELLED");
    h.post(&confirm("O1", 100_000)).await;
    let o = h.order("O1").await.unwrap();
    assert_eq!(o.status, "CANCELLED");
    assert_eq!(o.effective_amount, 0);
    assert_eq!(o.net_amount, 100_000);
    let n: u64 =
        h.db.ch
            .query("SELECT count() FROM orders FINAL")
            .fetch_one()
            .await
            .unwrap();
    assert_eq!(n, 1, "ordered_at이 바뀌어 파티션이 달라져도 FINAL은 한 행");
}

#[tokio::test(flavor = "multi_thread")]
async fn out_of_order_partial_refunds_and_late_confirm() {
    let h = setup().await;
    h.post(&refund("O1", 20_000, "2026-11-20T14:00:00Z")).await;
    assert_eq!(h.order("O1").await.unwrap().status, "PENDING");
    h.post(&refund("O1", 10_000, "2026-11-20T13:00:00Z")).await;
    h.post(&confirm("O1", 100_000)).await;
    let o = h.order("O1").await.unwrap();
    assert_eq!(o.status, "PARTIALLY_REFUNDED");
    assert_eq!(o.effective_amount, 70_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_requests_for_same_order_do_not_race() {
    let h = setup().await;
    h.post(&confirm("O1", 1_000_000)).await;
    let mut tasks = vec![];
    for i in 0..20 {
        let app = h.app.clone();
        let auth = format!("Bearer {}", h.key);
        let body = refund("O1", 1_000, &format!("2026-11-20T12:00:{i:02}Z")).to_string();
        tasks.push(tokio::spawn(async move {
            send(
                &app,
                "/v1/orders",
                &[("authorization", &auth)],
                body.into_bytes(),
            )
            .await
            .0
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), StatusCode::ACCEPTED);
    }
    let o = h.order("O1").await.unwrap();
    assert_eq!(o.refunded_amount, 20_000, "직렬 처리로 모든 환불이 반영");
    assert_eq!(o.effective_amount, 980_000);
    let versions: Vec<u64> =
        h.db.ch
            .query("SELECT version FROM orders WHERE order_id = 'O1' ORDER BY version")
            .fetch_all()
            .await
            .unwrap();
    let mut dedup = versions.clone();
    dedup.dedup();
    assert_eq!(versions, dedup, "version은 주문 안에서 겹치지 않는다");
}

#[tokio::test(flavor = "multi_thread")]
async fn same_ms_events_in_one_batch() {
    let h = setup().await;
    let at = "2026-11-20T12:00:00.000Z";
    let body = json!({"orders": [refund("O1", 1_000, at), refund("O1", 2_000, at), with(confirm("O1", 10_000), "ordered_at", json!(at))]});
    let (s, _) = h.batch(&body).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let o = h.order("O1").await.unwrap();
    assert_eq!(o.refunded_amount, 3_000);
    assert_eq!(o.effective_amount, 7_000);
}

// ── 외부 공격 ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn missing_or_invalid_keys_rejected() {
    let h = setup().await;
    let body = confirm("O1", 1).to_string();
    for headers in [
        vec![],
        vec![("authorization", "Basic abc")],
        vec![("authorization", "Bearer")],
        vec![("authorization", "Bearer sk_live_nope")],
        vec![("authorization", "Bearer pk_devlocal0000000000000000")],
    ] {
        let (s, v) = send(&h.app, "/v1/orders", &headers, body.clone().into_bytes()).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{headers:?}");
        assert!(v["error"].is_string());
    }
    // 같은 prefix의 위조 키
    let forged = format!(
        "Bearer {}{}",
        &h.key[..h.key.len() - 1],
        if h.key.ends_with('a') { 'b' } else { 'a' }
    );
    let (s, _) = send(
        &h.app,
        "/v1/orders",
        &[("authorization", &forged)],
        body.clone().into_bytes(),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // 소문자 bearer 스킴은 허용
    let lower = format!("bearer {}", h.key);
    let (s, _) = send(
        &h.app,
        "/v1/orders",
        &[("authorization", &lower)],
        body.into_bytes(),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
}

#[tokio::test(flavor = "multi_thread")]
async fn revoked_key_rejected() {
    let h = setup().await;
    let keys = analytics_pg::api_keys::list(&h.db.pg, h.project)
        .await
        .unwrap();
    analytics_pg::api_keys::revoke(&h.db.pg, h.project, keys[0].id)
        .await
        .unwrap();
    let (s, v) = h.post(&confirm("O1", 1)).await;
    assert_eq!((s, err_code(&v)), (StatusCode::UNAUTHORIZED, "invalid_key"));
    assert!(h.order("O1").await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn key_scopes_and_project_status() {
    let h = setup().await;
    sqlx::query("UPDATE api_keys SET scopes = ARRAY['reports:read']")
        .execute(&h.db.pg)
        .await
        .unwrap();
    let (s, v) = h.post(&confirm("O1", 1)).await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::FORBIDDEN, "insufficient_scope")
    );

    let h = setup().await;
    sqlx::query("UPDATE projects SET status = 'paused'")
        .execute(&h.db.pg)
        .await
        .unwrap();
    let (s, v) = h.post(&confirm("O1", 1)).await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::FORBIDDEN, "project_inactive")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn key_of_another_project_writes_only_there() {
    let h = setup().await;
    let (other, _) = make_project(&h.db, json!({})).await;
    let (other_key, _) = analytics_pg::api_keys::create(&h.db.pg, other, None)
        .await
        .unwrap();
    let auth = format!("Bearer {other_key}");
    let (s, _) = send(
        &h.app,
        "/v1/orders",
        &[("authorization", &auth)],
        confirm("SHARED", 5).to_string().into_bytes(),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert!(
        h.order("SHARED").await.is_none(),
        "A 프로젝트에는 쓰이지 않는다"
    );
    let in_other =
        ch_orders::current_order(&h.db.ch, ProjectScope::authorized(other as u32), "SHARED")
            .await
            .unwrap();
    assert!(in_other.is_some());
    // 본문에 project_id를 넣어도 키의 프로젝트가 우선한다
    let mut sneaky = confirm("SNEAKY", 1);
    sneaky["project_id"] = json!(h.project);
    send(
        &h.app,
        "/v1/orders",
        &[("authorization", &auth)],
        sneaky.to_string().into_bytes(),
    )
    .await;
    assert!(h.order("SNEAKY").await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_and_hostile_payloads() {
    let h = setup().await;
    for raw in [&b"{not json"[..], b"\xff\xfe", b"[1,2", b"null"] {
        let (s, v) = h.post_raw("/v1/orders", raw.to_vec()).await;
        assert!(
            s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY,
            "{raw:?} {s}"
        );
        assert!(v["error"].is_string());
    }
    let sql = "x'; DROP TABLE orders; -- ?";
    assert_eq!(h.post(&confirm(sql, 1)).await.0, StatusCode::ACCEPTED);
    assert_eq!(h.order(sql).await.unwrap().order_id, sql);

    let (s, v) = h.post(&confirm("ORD\n1", 1)).await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_order_id")
    );
    let (s, v) = h
        .post(&with(
            confirm("E1", 1),
            "member_id_hash",
            json!("user@example.com"),
        ))
        .await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_member_hash")
    );
    let (s, v) = h
        .post(&with(confirm("C1", 1), "currency", json!("USD")))
        .await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::UNPROCESSABLE_ENTITY, "currency_mismatch")
    );
    let (s, v) = h
        .post(&with(
            confirm("T1", 1),
            "ordered_at",
            json!("2099-01-01T00:00:00Z"),
        ))
        .await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::UNPROCESSABLE_ENTITY, "timestamp_out_of_range")
    );
    assert!(v["message"].is_string());
}

/// Worker와 같은 방식(v1)으로 `/internal/orders` 요청을 서명한다.
fn signed_for(
    secret: &[u8],
    body: &[u8],
    ts: i64,
    path: &str,
    auth: &str,
    idem: &str,
) -> (String, String) {
    let req = edge_auth::SignedRequest {
        internal_path: edge_auth::ORDERS_PATH,
        timestamp_ms: ts,
        body,
        forwarded_path: path,
        forwarded_authorization: auth,
        idempotency_key: idem,
    };
    (ts.to_string(), edge_auth::sign(secret, &req))
}

fn edge_headers(ts: &str, sig: &str, path: &str, auth: &str) -> Vec<(String, String)> {
    vec![
        ("x-edge-timestamp".to_owned(), ts.to_owned()),
        ("x-edge-signature".to_owned(), sig.to_owned()),
        ("x-forwarded-path".to_owned(), path.to_owned()),
        ("x-forwarded-authorization".to_owned(), auth.to_owned()),
    ]
}

async fn post_internal(app: &Router, headers: Vec<(String, String)>, b: Vec<u8>) -> StatusCode {
    let hs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    send(app, "/internal/orders", &hs, b).await.0
}

#[tokio::test(flavor = "multi_thread")]
async fn internal_signature_covers_forwarded_headers_and_path() {
    let h = setup().await;
    let body = confirm("X1", 1).to_string().into_bytes();
    let auth = format!("Bearer {}", h.key);
    let now = crate::internal::now_ms();

    // 단건 경로로 서명한 요청을 일괄 경로로 바꿔 보낼 수 없다
    let (ts, sig) = signed_for(SECRET, &body, now, "/v1/orders", &auth, "");
    let s = post_internal(
        &h.app,
        edge_headers(&ts, &sig, "/v1/orders:batch", &auth),
        body.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "x-forwarded-path 바꿔치기");
    // 다른 Authorization으로 바꿔치기
    let (ts, sig) = signed_for(SECRET, &body, now, "/v1/orders", "Bearer sk_live_a", "");
    let s = post_internal(
        &h.app,
        edge_headers(&ts, &sig, "/v1/orders", &auth),
        body.clone(),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::UNAUTHORIZED,
        "x-forwarded-authorization 바꿔치기"
    );
    // 서명되지 않은 멱등 키 추가
    let (ts, sig) = signed_for(SECRET, &body, now, "/v1/orders", &auth, "");
    let mut hs = edge_headers(&ts, &sig, "/v1/orders", &auth);
    hs.push(("idempotency-key".into(), "injected".into()));
    let s = post_internal(&h.app, hs, body.clone()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "idempotency-key 주입");
    // /internal/collect용 서명을 /internal/orders에 재생
    let csig = edge_auth::sign(SECRET, &edge_auth::SignedRequest::collect(now, &body));
    let s = post_internal(
        &h.app,
        edge_headers(&now.to_string(), &csig, "/v1/orders", &auth),
        body.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "교차 경로 재생");
    // (이전 형식 서명 거부는 core edge_auth 단위 테스트가 확인한다)
    // 극단 타임스탬프는 패닉 없이 401
    let (ts, sig) = signed_for(SECRET, &body, i64::MIN, "/v1/orders", &auth, "");
    let s = post_internal(
        &h.app,
        edge_headers(&ts, &sig, "/v1/orders", &auth),
        body.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(h.event_count("X1").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn internal_route_with_edge_signature() {
    let h = setup().await;
    let body = confirm("E1", 1234).to_string().into_bytes();
    let auth = format!("Bearer {}", h.key);
    let now = crate::internal::now_ms();
    let signed = |body: &[u8], ts: i64| signed_for(SECRET, body, ts, "/v1/orders", &auth, "");
    let (ts, sig) = signed(&body, now);
    let hdr = |ts: &str, sig: &str, path: &str, auth: &str| edge_headers(ts, sig, path, auth);
    let post = |headers: Vec<(String, String)>, b: Vec<u8>| {
        let app = h.app.clone();
        async move {
            let hs: Vec<(&str, &str)> = headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            send(&app, "/internal/orders", &hs, b).await
        }
    };

    let (s, v) = post(hdr(&ts, &sig, "/v1/orders", &auth), body.clone()).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert!(h.order("E1").await.is_some());

    // 서명 누락·위조·다른 본문·오래된 타임스탬프(재사용)
    let (s, _) = post(
        vec![
            ("x-forwarded-authorization".into(), auth.clone()),
            ("x-forwarded-path".into(), "/v1/orders".into()),
        ],
        body.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = post(hdr(&ts, &"0".repeat(64), "/v1/orders", &auth), body.clone()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let other = confirm("E2", 1).to_string().into_bytes();
    let (s, _) = post(hdr(&ts, &sig, "/v1/orders", &auth), other).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "다른 본문에 서명 재사용");
    let old = now - 10 * 60 * 1000;
    let (ots, osig) = signed(&body, old);
    let (s, _) = post(hdr(&ots, &osig, "/v1/orders", &auth), body.clone()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "오래된 서명 재사용");
    let (_, wrong) = signed_for(
        b"wrong-secret-wrong-secret-wrong-secret",
        &body,
        now,
        "/v1/orders",
        &auth,
        "",
    );
    let (s, _) = post(hdr(&ts, &wrong, "/v1/orders", &auth), body.clone()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // 서명이 맞아도 원 요청의 키는 원점에서 다시 검증한다
    let forged = "Bearer sk_live_forged";
    let (_, fsig) = signed_for(SECRET, &body, now, "/v1/orders", forged, "");
    let (s, v) = post(hdr(&ts, &fsig, "/v1/orders", forged), body.clone()).await;
    assert_eq!((s, err_code(&v)), (StatusCode::UNAUTHORIZED, "invalid_key"));
    let (_, esig) = signed_for(SECRET, &body, now, "/v1/orders", "", "");
    let (s, _) = post(hdr(&ts, &esig, "/v1/orders", ""), body.clone()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // 모르는 경로
    let (_, usig) = signed_for(SECRET, &body, now, "/v1/collect", &auth, "");
    let (s, _) = post(hdr(&ts, &usig, "/v1/collect", &auth), body.clone()).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // 일괄 + 엣지가 전달한 idempotency-key
    let batch = json!({"orders": [confirm("E3", 1)]})
        .to_string()
        .into_bytes();
    let (bts, bsig) = signed_for(SECRET, &batch, now, "/v1/orders:batch", &auth, "");
    let (s, v) = post(hdr(&bts, &bsig, "/v1/orders:batch", &auth), batch).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(v["results"][0]["accepted"], true);

    let r = refund("E1", 5, "2026-11-20T12:00:00Z")
        .to_string()
        .into_bytes();
    let (rts, rsig) = signed_for(SECRET, &r, now, "/v1/orders", &auth, "edge-key-1");
    let mut with_key = hdr(&rts, &rsig, "/v1/orders", &auth);
    with_key.push(("idempotency-key".into(), "edge-key-1".into()));
    let (s, _) = post(with_key, r).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let evs = ch_orders::order_events(&h.db.ch, h.scope(), "E1")
        .await
        .unwrap();
    assert!(evs.iter().any(|e| e.idempotency_key == "edge-key-1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn internal_route_disabled_without_secret() {
    let h = setup().await;
    let app = router(OrdersHttp {
        svc: h.svc.clone(),
        edge_secret: Arc::new(vec![]),
        emulation: false,
    });
    let (s, _) = send(&app, "/internal/orders", &[], b"{}".to_vec()).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = send(&app, "/v1/orders", &[], b"{}".to_vec()).await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "운영 모드에서는 /v1/orders를 직접 받지 않는다"
    );
}

// ── 장애·복구 ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn clickhouse_down_returns_503_and_stores_nothing() {
    let h = setup().await;
    let dead = analytics_ch::Client::default().with_url("http://127.0.0.1:1");
    let app = app_for(Arc::new(OrderService::new(h.db.pg.clone(), dead)));
    let auth = format!("Bearer {}", h.key);
    let (s, v) = send(
        &app,
        "/v1/orders",
        &[("authorization", &auth)],
        confirm("O1", 1).to_string().into_bytes(),
    )
    .await;
    assert_eq!(
        (s, err_code(&v)),
        (StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable")
    );
    assert_ne!(v["accepted"], true);
    let (s, v) = send(
        &app,
        "/v1/orders:batch",
        &[("authorization", &auth)],
        json!([confirm("O2", 1)]).to_string().into_bytes(),
    )
    .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(v["results"][0]["accepted"], false);
    assert_eq!(v["results"][0]["error"], "storage_unavailable");
    assert_eq!(h.event_count("O1").await + h.event_count("O2").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn projection_failure_is_5xx_and_retry_recovers() {
    let h = setup().await;
    h.db.ch
        .query("RENAME TABLE orders TO orders_away")
        .execute()
        .await
        .unwrap();
    let (s, _) = h.post(&confirm("O1", 100_000)).await;
    assert_eq!(
        s,
        StatusCode::SERVICE_UNAVAILABLE,
        "투영 실패를 성공으로 보고하지 않는다"
    );
    h.db.ch
        .query("RENAME TABLE orders_away TO orders")
        .execute()
        .await
        .unwrap();
    assert!(h.order("O1").await.is_none());
    // 고객사 재시도: 이벤트는 이미 있지만 투영을 마저 쓴다
    let (s, _) = h.post(&confirm("O1", 100_000)).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(h.order("O1").await.unwrap().effective_amount, 100_000);
    assert_eq!(h.event_count("O1").await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn event_append_failure_writes_no_projection() {
    let h = setup().await;
    h.db.ch
        .query("RENAME TABLE order_events TO oe_away")
        .execute()
        .await
        .unwrap();
    let (s, _) = h.post(&confirm("O1", 1)).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert!(h.order("O1").await.is_none());
}

// ── 클라이언트 purchase → 주문 ─────────────────────────────

fn purchase(project: i32, order_id: &str, value: i64, event_id: u128) -> analytics_core::Event {
    let raw = json!({
        "project_id": project, "event_id": uuid::Uuid::from_u128(event_id), "ts_ms": 1_795_167_912_000i64,
        "received_at_ms": 1_795_167_913_000i64, "event_name": "purchase",
        "visitor_id": uuid::Uuid::from_u128(77), "session_id": uuid::Uuid::from_u128(3),
        "is_session_start": false, "member_hash": "", "url": "https://a.com/done", "host": "a.com",
        "path": "/done", "title": "", "referrer": "", "referrer_host": "", "utm_source": "",
        "utm_medium": "", "utm_campaign": "", "utm_content": "", "utm_term": "", "click_id": "",
        "click_id_type": "", "channel": "direct", "country": "KR", "region": "", "city": "",
        "device_type": "desktop", "browser": "", "os": "", "screen_w": 0, "props": [],
        "ec": { "order_id": order_id, "value": value, "currency": "KRW", "items": [] }
    });
    serde_json::from_value(raw).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn client_purchase_projected_when_enabled() {
    let h = setup_with(json!({"client_purchase_as_order": true})).await;
    let e = purchase(h.project, "C1", 50_000, 1);
    h.svc.client_purchases(&[e.clone(), e]).await.unwrap();
    let o = h.order("C1").await.unwrap();
    assert_eq!(o.source, "client");
    assert_eq!(o.effective_amount, 50_000);
    assert_eq!(o.visitor_id, Some(uuid::Uuid::from_u128(77)));
    assert_eq!(h.event_count("C1").await, 1, "재전송은 한 번만");
}

#[tokio::test(flavor = "multi_thread")]
async fn client_purchase_ignored_when_disabled() {
    let h = setup().await;
    h.svc
        .client_purchases(&[purchase(h.project, "C1", 50_000, 1)])
        .await
        .unwrap();
    assert!(h.order("C1").await.is_none());
    assert_eq!(h.event_count("C1").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn s2s_always_wins_over_client() {
    let h = setup_with(json!({"client_purchase_as_order": true})).await;
    // S2S 먼저 → 클라이언트는 덮어쓰지 못한다
    h.post(&confirm("O1", 100_000)).await;
    let before = h.order("O1").await.unwrap();
    h.svc
        .client_purchases(&[purchase(h.project, "O1", 999, 1)])
        .await
        .unwrap();
    assert_eq!(h.order("O1").await.unwrap(), before);
    // 클라이언트 먼저 → S2S가 오면 S2S로 바뀐다
    h.svc
        .client_purchases(&[purchase(h.project, "O2", 999, 2)])
        .await
        .unwrap();
    assert_eq!(h.order("O2").await.unwrap().source, "client");
    h.post(&confirm("O2", 100_000)).await;
    let o = h.order("O2").await.unwrap();
    assert_eq!(o.source, "s2s");
    assert_eq!(o.net_amount, 100_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn project_context_lookup() {
    let h = setup().await;
    let ctx: ProjectCtx = h.svc.project(h.project as u32).await.unwrap().unwrap();
    assert!(ctx.active);
    assert_eq!(ctx.currency, "KRW");
    assert_eq!(ctx.salt_hex, h.salt);
    assert!(!ctx.client_purchase_as_order);
    assert!(h.svc.project(999_999).await.unwrap().is_none());
    let _ = OrderStatus::Confirmed;
}

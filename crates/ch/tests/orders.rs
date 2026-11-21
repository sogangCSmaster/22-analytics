//! `order_events` / `orders` 통합 테스트 (격리 ClickHouse DB).

use analytics_ch::ProjectScope;
use analytics_ch::attribution;
use analytics_ch::orders::{self, OrderRow};
use analytics_core::orders::{OrderEvent, OrderEventType, OrderSource, OrderState, OrderStatus};
use analytics_testkit::TestDb;
use uuid::Uuid;

/// 2026-11-20T09:45:12Z
const T: i64 = 1_795_167_912_000;

fn ev(order_id: &str, key: &str, t: OrderEventType) -> OrderEvent {
    OrderEvent {
        project_id: 1,
        order_id: order_id.into(),
        event_type: t,
        source: OrderSource::S2s,
        idempotency_key: key.into(),
        visitor_id: Some(Uuid::from_u128(42)),
        member_hash: "mh".into(),
        currency: "KRW".into(),
        gross_amount: 110_000,
        net_amount: 100_000,
        refund_amount: 0,
        occurred_at_ms: T + 123,
        received_at_ms: T + 456,
        raw: r#"{"x":"원본"}"#.into(),
    }
}

fn state(order_id: &str, ordered_at_ms: i64) -> OrderState {
    OrderState {
        project_id: 1,
        order_id: order_id.into(),
        source: OrderSource::S2s,
        status: OrderStatus::Confirmed,
        visitor_id: None,
        member_hash: String::new(),
        currency: "KRW".into(),
        gross_amount: 100,
        net_amount: 100,
        refunded_amount: 0,
        effective_amount: 100,
        ordered_at_ms,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn order_event_roundtrip() {
    let db = TestDb::new().await;
    let scope = ProjectScope::authorized(1);
    let mut e = ev("O1", "k1", OrderEventType::PurchaseConfirmed);
    orders::insert_order_event(&db.ch, &e).await.unwrap();
    e.visitor_id = None;
    e.idempotency_key = "k2".into();
    orders::insert_order_event(&db.ch, &e).await.unwrap();

    let mut got = orders::order_events(&db.ch, scope, "O1").await.unwrap();
    got.sort_by(|a, b| a.idempotency_key.cmp(&b.idempotency_key));
    assert_eq!(got.len(), 2);
    assert_eq!(got[0], ev("O1", "k1", OrderEventType::PurchaseConfirmed));
    assert_eq!(got[1], e, "Nullable visitor_id None 왕복");
    assert!(
        orders::order_events(&db.ch, scope, "nope")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_rows_are_all_returned_for_fold_dedupe() {
    let db = TestDb::new().await;
    let e = ev("O1", "k1", OrderEventType::OrderRefunded);
    orders::insert_order_event(&db.ch, &e).await.unwrap();
    orders::insert_order_event(&db.ch, &e).await.unwrap();
    let got = orders::order_events(&db.ch, ProjectScope::authorized(1), "O1")
        .await
        .unwrap();
    assert!(!got.is_empty());
    let s = analytics_core::orders::fold(&got).unwrap();
    assert_eq!(s.status, OrderStatus::Pending);
}

#[tokio::test(flavor = "multi_thread")]
async fn orders_final_returns_latest_version() {
    let db = TestDb::new().await;
    let scope = ProjectScope::authorized(1);
    assert!(
        orders::current_order(&db.ch, scope, "O1")
            .await
            .unwrap()
            .is_none()
    );
    let s = state("O1", T);
    orders::insert_order(&db.ch, &OrderRow::from_state(&s, 10))
        .await
        .unwrap();
    let mut cancelled = s.clone();
    cancelled.status = OrderStatus::Cancelled;
    cancelled.effective_amount = 0;
    orders::insert_order(&db.ch, &OrderRow::from_state(&cancelled, 11))
        .await
        .unwrap();
    // 낮은 version이 늦게 와도 높은 version이 이긴다
    orders::insert_order(&db.ch, &OrderRow::from_state(&s, 9))
        .await
        .unwrap();

    let row = orders::current_order(&db.ch, scope, "O1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.version, 11);
    assert_eq!(row.to_state().unwrap(), cancelled);
}

#[tokio::test(flavor = "multi_thread")]
async fn final_dedupes_across_partitions() {
    // 취소만 먼저 와서 12월로 투영됐다가, 확정이 와서 ordered_at이 11월로 바뀌는 경우.
    let db = TestDb::new().await;
    let scope = ProjectScope::authorized(1);
    let dec = T + 20 * 86_400_000;
    orders::insert_order(&db.ch, &OrderRow::from_state(&state("O1", dec), 1))
        .await
        .unwrap();
    orders::insert_order(&db.ch, &OrderRow::from_state(&state("O1", T), 2))
        .await
        .unwrap();
    let row = orders::current_order(&db.ch, scope, "O1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.version, 2);
    let changed = attribution::changed_orders(&db.ch, scope, 0).await.unwrap();
    assert_eq!(changed.len(), 1, "파티션이 달라도 FINAL은 한 행");
}

#[tokio::test(flavor = "multi_thread")]
async fn tenant_isolation() {
    let db = TestDb::new().await;
    let mut other = ev("O1", "k1", OrderEventType::PurchaseConfirmed);
    other.project_id = 2;
    orders::insert_order_event(&db.ch, &other).await.unwrap();
    let mut s = state("O1", T);
    s.project_id = 2;
    orders::insert_order(&db.ch, &OrderRow::from_state(&s, 1))
        .await
        .unwrap();

    let p1 = ProjectScope::authorized(1);
    assert!(
        orders::order_events(&db.ch, p1, "O1")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        orders::current_order(&db.ch, p1, "O1")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        attribution::changed_orders(&db.ch, p1, 0)
            .await
            .unwrap()
            .is_empty()
    );
    let p2 = ProjectScope::authorized(2);
    assert_eq!(
        orders::order_events(&db.ch, p2, "O1").await.unwrap().len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hostile_order_ids_stored_verbatim() {
    let db = TestDb::new().await;
    let scope = ProjectScope::authorized(1);
    for id in [
        "x'; DROP TABLE orders; --",
        "?",
        "a ? b ?? c",
        "back\\slash",
        "\"quoted\"",
        "주문-🛒",
        &"가".repeat(128),
    ] {
        orders::insert_order_event(&db.ch, &ev(id, "k", OrderEventType::PurchaseConfirmed))
            .await
            .unwrap();
        orders::insert_order(&db.ch, &OrderRow::from_state(&state(id, T), 1))
            .await
            .unwrap();
        let got = orders::order_events(&db.ch, scope, id).await.unwrap();
        assert_eq!(got.len(), 1, "{id}");
        assert_eq!(got[0].order_id, id);
        let row = orders::current_order(&db.ch, scope, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.order_id, id);
    }
    let n: u64 = db
        .ch
        .query("SELECT count() FROM orders FINAL")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(n, 7, "테이블이 남아 있고 모든 행이 그대로");
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_since_watermark() {
    let db = TestDb::new().await;
    let scope = ProjectScope::authorized(1);
    orders::insert_order(&db.ch, &OrderRow::from_state(&state("A", T), 100))
        .await
        .unwrap();
    orders::insert_order(&db.ch, &OrderRow::from_state(&state("B", T), 200))
        .await
        .unwrap();
    orders::insert_order(&db.ch, &OrderRow::from_state(&state("A", T), 300))
        .await
        .unwrap();
    let ids = |rows: Vec<attribution::OrderForAttribution>| {
        rows.into_iter().map(|r| r.order_id).collect::<Vec<_>>()
    };
    assert_eq!(
        ids(attribution::changed_orders(&db.ch, scope, 0).await.unwrap()),
        ["B", "A"]
    );
    assert_eq!(
        ids(attribution::changed_orders(&db.ch, scope, 200)
            .await
            .unwrap()),
        ["A"]
    );
    assert!(
        attribution::changed_orders(&db.ch, scope, 300)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_into_missing_table_fails() {
    let db = TestDb::new().await;
    db.ch
        .query("DROP TABLE order_events")
        .execute()
        .await
        .unwrap();
    let r =
        orders::insert_order_event(&db.ch, &ev("O1", "k", OrderEventType::PurchaseConfirmed)).await;
    assert!(r.is_err(), "insert 실패는 에러로 드러나야 한다");
}

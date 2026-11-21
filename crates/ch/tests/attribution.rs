//! 기여 연산 쿼리 통합 테스트 (격리된 ClickHouse DB).

use analytics_ch::ProjectScope;
use analytics_ch::attribution::{self as a, AttributedRow, fixtures::*};
use analytics_core::orders::OrderStatus;
use analytics_testkit::TestDb;
use uuid::Uuid;

const P: u32 = 1;
const OTHER: u32 = 2;

fn scope(p: u32) -> ProjectScope {
    ProjectScope::authorized(p)
}

fn row(order: &str, model: &str, idx: u16, amount: i64, computed_at: i64) -> AttributedRow {
    AttributedRow {
        project_id: P,
        model: model.into(),
        lookback_days: 7,
        order_id: order.into(),
        touch_index: idx,
        channel: if idx == 0 { "(unattributed)" } else { "meta" }.into(),
        utm_campaign: String::new(),
        touch_ts: (idx > 0).then_some(1_000 + idx as i64),
        ordered_at: 1_700_000_000_000,
        amount,
        is_assist: 0,
        computed_at,
        is_deleted: 0,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_orders_reads_latest_version_above_watermark() {
    let db = TestDb::new().await;
    let v = Uuid::new_v4();
    insert_orders(
        &db.ch,
        &[
            OrderFixture::confirmed(P, "A", Some(v), 1_000, 10, 100),
            OrderFixture::confirmed(P, "B", None, 2_000, 20, 200),
            OrderFixture::confirmed(P, "C", None, 3_000, 30, 50),
            // 다른 프로젝트의 같은 주문 ID
            OrderFixture::confirmed(OTHER, "A", Some(v), 9_999, 10, 999),
        ],
    )
    .await
    .unwrap();
    // A의 새 버전 (부분 환불)
    let mut a2 = OrderFixture::confirmed(P, "A", Some(v), 1_000, 10, 300);
    a2.status = OrderStatus::PartiallyRefunded.as_str().into();
    a2.refunded_amount = 400;
    a2.effective_amount = 600;
    insert_orders(&db.ch, &[a2]).await.unwrap();

    let got = a::changed_orders(&db.ch, scope(P), 60).await.unwrap();
    let ids: Vec<(&str, u64, i64)> = got
        .iter()
        .map(|o| (o.order_id.as_str(), o.version, o.effective_amount))
        .collect();
    assert_eq!(ids, vec![("B", 200, 2_000), ("A", 300, 600)]);
    assert_eq!(got[1].visitor_id, Some(v));
    assert_eq!(got[1].status, OrderStatus::PartiallyRefunded);
    assert_eq!(got[1].ordered_at_ms, 10);
    assert_eq!(got[0].visitor_id, None);

    assert!(
        a::changed_orders(&db.ch, scope(P), 300)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn orders_since_and_by_ids() {
    let db = TestDb::new().await;
    insert_orders(
        &db.ch,
        &[
            OrderFixture::confirmed(P, "old", None, 1, 1_000, 1),
            OrderFixture::confirmed(P, "edge", None, 1, 5_000, 2),
            OrderFixture::confirmed(P, "new", None, 1, 9_000, 3),
            OrderFixture::confirmed(OTHER, "new2", None, 1, 9_000, 3),
        ],
    )
    .await
    .unwrap();
    let since = a::orders_since(&db.ch, scope(P), 5_000).await.unwrap();
    assert_eq!(
        since
            .iter()
            .map(|o| o.order_id.as_str())
            .collect::<Vec<_>>(),
        vec!["edge", "new"]
    );
    let by = a::orders_by_ids(
        &db.ch,
        scope(P),
        &[
            "new".into(),
            "missing".into(),
            "new2".into(),
            "x'; DROP".into(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(by.len(), 1);
    assert_eq!(by[0].order_id, "new");
    assert!(
        a::orders_by_ids(&db.ch, scope(P), &[])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn identity_links_by_member_hash() {
    let db = TestDb::new().await;
    let (v1, v2, v3) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    insert_links(
        &db.ch,
        &[
            IdentityFixture::new(P, "m1", v1),
            IdentityFixture::new(P, "m1", v2),
            IdentityFixture::new(P, "m2", v3),
            IdentityFixture::new(OTHER, "m1", v3),
        ],
    )
    .await
    .unwrap();
    // 같은 링크가 두 번 들어와도 한 번만
    insert_links(&db.ch, &[IdentityFixture::new(P, "m1", v1)])
        .await
        .unwrap();

    let mut got = a::identity_links(&db.ch, scope(P), &["m1".into(), "".into(), "zz".into()])
        .await
        .unwrap();
    got.sort_by_key(|l| l.visitor_id);
    let mut want = vec![v1, v2];
    want.sort();
    assert_eq!(got.iter().map(|l| l.visitor_id).collect::<Vec<_>>(), want);
    assert!(got.iter().all(|l| l.member_hash == "m1"));
    assert!(
        a::identity_links(&db.ch, scope(P), &["".into()])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn touchpoints_window_is_inclusive_and_scoped() {
    let db = TestDb::new().await;
    let (v1, v2, stranger) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    insert_touches(
        &db.ch,
        &[
            TouchFixture::utm(P, v1, 999, "early", "", ""),
            TouchFixture::utm(P, v1, 1_000, "from_edge", "", ""),
            TouchFixture::utm(P, v2, 1_500, "mid", "cpc", "c1"),
            TouchFixture::utm(P, v1, 2_000, "to_edge", "", ""),
            TouchFixture::utm(P, v1, 2_001, "late", "", ""),
            TouchFixture::utm(P, stranger, 1_500, "stranger", "", ""),
            TouchFixture::utm(OTHER, v1, 1_500, "other_project", "", ""),
        ],
    )
    .await
    .unwrap();
    let got = a::touchpoints(&db.ch, scope(P), &[v1, v2], 1_000, 2_000)
        .await
        .unwrap();
    assert_eq!(
        got.iter()
            .map(|t| t.utm_source.as_str())
            .collect::<Vec<_>>(),
        vec!["from_edge", "mid", "to_edge"]
    );
    assert_eq!(got[1].visitor_id, v2);
    assert_eq!(got[1].utm_medium, "cpc");
    assert_eq!(got[1].utm_campaign, "c1");
    assert_eq!(got[1].page_host, "shop.example.com");
    assert_eq!(got[1].ts_ms, 1_500);
    assert!(
        a::touchpoints(&db.ch, scope(P), &[], 0, i64::MAX / 2)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn touchpoints_many_visitors_are_chunked() {
    let db = TestDb::new().await;
    let ids: Vec<Uuid> = (0..(a::IN_CHUNK * 2 + 7)).map(|_| Uuid::new_v4()).collect();
    let rows: Vec<TouchFixture> = ids
        .iter()
        .enumerate()
        .map(|(i, v)| TouchFixture::utm(P, *v, 1_000 + i as i64, "naver", "cpc", ""))
        .collect();
    insert_touches(&db.ch, &rows).await.unwrap();
    let got = a::touchpoints(&db.ch, scope(P), &ids, 0, 1_000_000)
        .await
        .unwrap();
    assert_eq!(got.len(), ids.len());
    // 청크를 나눠도 전체가 시간순
    assert!(got.windows(2).all(|w| w[0].ts_ms <= w[1].ts_ms));
}

#[tokio::test(flavor = "multi_thread")]
async fn latest_rows_returns_newest_including_deleted() {
    let db = TestDb::new().await;
    a::insert_rows(
        &db.ch,
        scope(P),
        &[
            row("o1", "linear", 1, 100, 10),
            row("o1", "linear", 2, 0, 10),
        ],
    )
    .await
    .unwrap();
    let mut del = row("o1", "linear", 2, 0, 20);
    del.is_deleted = 1;
    a::insert_rows(&db.ch, scope(P), &[row("o1", "linear", 1, 70, 20), del])
        .await
        .unwrap();
    // 다른 프로젝트 같은 주문 ID
    let mut foreign = row("o1", "linear", 1, 5, 99);
    foreign.project_id = OTHER;
    a::insert_rows(&db.ch, scope(OTHER), &[foreign])
        .await
        .unwrap();

    let mut got = a::latest_rows(&db.ch, scope(P), &["o1".into()])
        .await
        .unwrap();
    got.sort_by_key(|r| r.touch_index);
    assert_eq!(got.len(), 2);
    assert_eq!(
        (got[0].amount, got[0].computed_at, got[0].is_deleted),
        (70, 20, 0)
    );
    assert_eq!((got[1].computed_at, got[1].is_deleted), (20, 1));
    assert_eq!(got[0].touch_ts, Some(1_001));
    assert_eq!(got[0].ordered_at, 1_700_000_000_000);
    assert!(
        a::latest_rows(&db.ch, scope(P), &[])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unattributed_row_has_null_touch_ts() {
    let db = TestDb::new().await;
    a::insert_rows(&db.ch, scope(P), &[row("o1", "last_click", 0, 100, 1)])
        .await
        .unwrap();
    let got = a::latest_rows(&db.ch, scope(P), &["o1".into()])
        .await
        .unwrap();
    assert_eq!(got[0].touch_ts, None);
    let null: u64 = db
        .ch
        .query("SELECT count() FROM attributed_revenue WHERE touch_ts IS NULL")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(null, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_rejects_rows_of_another_project() {
    let db = TestDb::new().await;
    let mut r = row("o1", "linear", 1, 100, 1);
    r.project_id = OTHER;
    assert!(a::insert_rows(&db.ch, scope(P), &[r]).await.is_err());
    let n: u64 = db
        .ch
        .query("SELECT count() FROM attributed_revenue")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(n, 0);
    // 빈 입력은 아무것도 하지 않는다
    a::insert_rows(&db.ch, scope(P), &[]).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn conservation_check_finds_broken_orders_only() {
    let db = TestDb::new().await;
    insert_orders(
        &db.ch,
        &[
            OrderFixture::confirmed(P, "ok", None, 100, 1_700_000_000_000, 1),
            OrderFixture::confirmed(P, "bad", None, 100, 1_700_000_000_000, 1),
            OrderFixture::confirmed(OTHER, "bad", None, 100, 1_700_000_000_000, 1),
        ],
    )
    .await
    .unwrap();
    let mut rows = vec![
        row("ok", "linear", 1, 60, 1),
        row("ok", "linear", 2, 40, 1),
        // raw_sum은 보존 대상이 아니다
        row("ok", "raw_sum", 1, 100, 1),
        row("ok", "raw_sum", 2, 100, 1),
        row("bad", "linear", 1, 60, 1),
        row("bad", "linear", 2, 40, 1),
    ];
    // 삭제 표시된 행은 합계에서 빠진다
    let mut gone = row("ok", "linear", 3, 999, 1);
    gone.is_deleted = 1;
    rows.push(gone);
    a::insert_rows(&db.ch, scope(P), &rows).await.unwrap();
    assert!(
        a::conservation_check(&db.ch, scope(P))
            .await
            .unwrap()
            .is_empty()
    );

    // 의도적으로 손상: 더 새 computed_at으로 금액을 바꾼다
    a::insert_rows(&db.ch, scope(P), &[row("bad", "linear", 2, 41, 2)])
        .await
        .unwrap();
    let broken = a::conservation_check(&db.ch, scope(P)).await.unwrap();
    assert_eq!(
        broken,
        vec![a::BrokenOrder {
            model: "linear".into(),
            lookback_days: 7,
            order_id: "bad".into(),
            attributed: 101,
            effective_amount: 100,
        }]
    );
    // 다른 프로젝트는 영향 없음
    assert!(
        a::conservation_check(&db.ch, scope(OTHER))
            .await
            .unwrap()
            .is_empty()
    );
}

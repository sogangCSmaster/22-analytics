//! 개요·추이·채널·페이지 리포트.

use crate::common::*;
use analytics_ch::reports::{self, DateRange, MAX_LIMIT};
use analytics_core::dto::reports::*;
use analytics_testkit::TestDb;

fn range(from: &str, to: &str) -> DateRange {
    DateRange::parse(from, to).unwrap()
}

/// 기본 트래픽 픽스처 (프로젝트 P).
///
/// 11-20: v1/s11 naver_sa  10:00 pv "/" · 10:01 pv "/goods/a" · 10:02 add_to_cart "/goods/a"
///        v2/s21 meta      11:00 pv "/" · 11:05 pv "/cart"
///        v1/s12 direct    20:00 pv "/"
/// 11-21: v3/s31 naver_sa  09:00 pv "/goods/a"
/// 11-19: v4/s41 meta      12:00 pv "/"           (직전 기간)
async fn traffic_fixture(db: &TestDb, project: u32) {
    insert(
        &db.ch,
        vec![
            ev(project, "page_view", 1, 11, "2026-11-20 10:00:00").start("naver_sa"),
            ev(project, "page_view", 1, 11, "2026-11-20 10:01:00")
                .ch("naver_sa")
                .path("/goods/a"),
            ev(project, "add_to_cart", 1, 11, "2026-11-20 10:02:00")
                .ch("naver_sa")
                .path("/goods/a"),
            ev(project, "page_view", 2, 21, "2026-11-20 11:00:00").start("meta"),
            ev(project, "page_view", 2, 21, "2026-11-20 11:05:00")
                .ch("meta")
                .path("/cart"),
            ev(project, "page_view", 1, 12, "2026-11-20 20:00:00").start("direct"),
            ev(project, "page_view", 3, 31, "2026-11-21 09:00:00")
                .start("naver_sa")
                .path("/goods/a"),
            ev(project, "page_view", 4, 41, "2026-11-19 12:00:00").start("meta"),
        ],
    )
    .await;
}

/// 다른 테넌트(Q)에만 들어가는 큰 값.
async fn other_tenant_fixture(db: &TestDb) {
    let mut evs = Vec::new();
    for v in 0..20u128 {
        evs.push(
            ev(Q, "page_view", 1000 + v, 2000 + v, "2026-11-20 12:00:00")
                .start("kakao")
                .path("/q-only"),
        );
    }
    insert(&db.ch, evs).await;
    insert_orders(
        &db.ch,
        vec![order(
            Q,
            "Q-1",
            "CONFIRMED",
            999_000,
            0,
            "2026-11-20 12:00:00",
            1,
        )],
    )
    .await;
}

// ------------------------------------------------------------------ overview

#[tokio::test]
async fn overview_happy_with_previous_period_and_revenue() {
    let db = TestDb::new().await;
    traffic_fixture(&db, P).await;
    other_tenant_fixture(&db).await;
    insert_orders(
        &db.ch,
        vec![
            order(P, "A", "CONFIRMED", 100_000, 0, "2026-11-20 18:45:00", 1),
            order(
                P,
                "B",
                "PARTIALLY_REFUNDED",
                40_000,
                10_000,
                "2026-11-21 10:00:00",
                1,
            ),
            // 취소 주문은 제외
            order(
                P,
                "C",
                "CANCELLED",
                50_000,
                50_000,
                "2026-11-21 11:00:00",
                1,
            ),
            // 직전 기간
            order(P, "D", "CONFIRMED", 7_000, 0, "2026-11-18 09:00:00", 1),
            // 기간 밖
            order(P, "E", "CONFIRMED", 1_000_000, 0, "2026-11-22 00:00:00", 1),
        ],
    )
    .await;

    let r = reports::overview(&db.ch, scope(P), range("2026-11-20", "2026-11-21"))
        .await
        .unwrap();
    assert_eq!(r.range.from, "2026-11-20");
    assert_eq!(r.previous_range.from, "2026-11-18");
    assert_eq!(r.previous_range.to, "2026-11-19");
    assert_eq!(
        r.current,
        OverviewMetrics {
            visitors: 3,
            sessions: 4,
            pageviews: 6,
            entrances: 4,
            events: 7,
            revenue: 130_000,
            orders: 2,
        }
    );
    assert_eq!(
        r.previous,
        OverviewMetrics {
            visitors: 1,
            sessions: 1,
            pageviews: 1,
            entrances: 1,
            events: 1,
            revenue: 7_000,
            orders: 1,
        }
    );
}

#[tokio::test]
async fn overview_is_idempotent() {
    let db = TestDb::new().await;
    traffic_fixture(&db, P).await;
    let r = range("2026-11-20", "2026-11-21");
    let a = reports::overview(&db.ch, scope(P), r).await.unwrap();
    let b = reports::overview(&db.ch, scope(P), r).await.unwrap();
    assert_eq!(a, b);
}

#[tokio::test]
async fn overview_empty_project_is_zero() {
    let db = TestDb::new().await;
    let r = reports::overview(&db.ch, scope(P), range("2026-11-20", "2026-11-21"))
        .await
        .unwrap();
    assert_eq!(r.current, OverviewMetrics::default());
    assert_eq!(r.previous, OverviewMetrics::default());
}

#[tokio::test]
async fn overview_order_versions_use_latest() {
    let db = TestDb::new().await;
    insert_orders(
        &db.ch,
        vec![order(
            P,
            "A",
            "CONFIRMED",
            100_000,
            0,
            "2026-11-20 10:00:00",
            1,
        )],
    )
    .await;
    // 나중 버전: 취소됨 → 매출에서 빠져야 한다
    insert_orders(
        &db.ch,
        vec![order(
            P,
            "A",
            "CANCELLED",
            100_000,
            100_000,
            "2026-11-20 10:00:00",
            2,
        )],
    )
    .await;
    insert_orders(
        &db.ch,
        vec![
            order(P, "B", "CONFIRMED", 5_000, 0, "2026-11-20 10:00:00", 1),
            order(
                P,
                "B",
                "PARTIALLY_REFUNDED",
                5_000,
                2_000,
                "2026-11-20 10:00:00",
                2,
            ),
        ],
    )
    .await;
    let r = reports::overview(
        &db.ch,
        scope(P),
        DateRange::parse("2026-11-20", "2026-11-20").unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(r.current.revenue, 3_000);
    assert_eq!(r.current.orders, 1);
}

#[tokio::test]
async fn overview_timezone_boundary() {
    let db = TestDb::new().await;
    insert(
        &db.ch,
        vec![
            ev(P, "page_view", 1, 1, "2026-11-20 23:59:59").start("direct"),
            ev(P, "page_view", 2, 2, "2026-11-21 00:00:00").start("direct"),
        ],
    )
    .await;
    insert_orders(
        &db.ch,
        vec![
            order(P, "late", "CONFIRMED", 1_000, 0, "2026-11-20 23:59:59", 1),
            order(P, "next", "CONFIRMED", 2_000, 0, "2026-11-21 00:00:00", 1),
        ],
    )
    .await;
    let d20 = reports::overview(&db.ch, scope(P), range("2026-11-20", "2026-11-20"))
        .await
        .unwrap();
    assert_eq!((d20.current.visitors, d20.current.pageviews), (1, 1));
    assert_eq!(d20.current.revenue, 1_000);
    let d21 = reports::overview(&db.ch, scope(P), range("2026-11-21", "2026-11-21"))
        .await
        .unwrap();
    assert_eq!((d21.current.visitors, d21.current.pageviews), (1, 1));
    assert_eq!(d21.current.revenue, 2_000);
    // 11-21 기준 직전 기간 = 11-20
    assert_eq!(d21.previous.pageviews, 1);
}

#[tokio::test]
async fn overview_range_crossing_month_partitions() {
    let db = TestDb::new().await;
    insert(
        &db.ch,
        vec![
            ev(P, "page_view", 1, 1, "2026-10-31 12:00:00").start("direct"),
            // KST 11-01 00:30 = UTC 10-31 15:30 (UTC 기준으로는 10월 파티션)
            ev(P, "page_view", 2, 2, "2026-11-01 00:30:00").start("direct"),
            ev(P, "page_view", 3, 3, "2026-11-01 12:00:00").start("direct"),
            ev(P, "page_view", 4, 4, "2026-11-02 12:00:00").start("direct"),
        ],
    )
    .await;
    let r = reports::overview(&db.ch, scope(P), range("2026-10-31", "2026-11-01"))
        .await
        .unwrap();
    assert_eq!(r.current.pageviews, 3);
    assert_eq!(r.current.visitors, 3);
}

#[tokio::test]
async fn overview_tenant_isolation() {
    let db = TestDb::new().await;
    other_tenant_fixture(&db).await;
    let r = range("2026-11-20", "2026-11-20");
    let p = reports::overview(&db.ch, scope(P), r).await.unwrap();
    assert_eq!(p.current, OverviewMetrics::default());
    let q = reports::overview(&db.ch, scope(Q), r).await.unwrap();
    assert_eq!(q.current.visitors, 20);
    assert_eq!(q.current.revenue, 999_000);
}

// ------------------------------------------------------------------ timeseries

#[tokio::test]
async fn timeseries_fills_missing_days() {
    let db = TestDb::new().await;
    traffic_fixture(&db, P).await;
    other_tenant_fixture(&db).await;
    let r = reports::timeseries(&db.ch, scope(P), range("2026-11-18", "2026-11-22"))
        .await
        .unwrap();
    let got: Vec<(&str, u64, u64, u64)> = r
        .points
        .iter()
        .map(|p| (p.date.as_str(), p.visitors, p.sessions, p.pageviews))
        .collect();
    assert_eq!(
        got,
        vec![
            ("2026-11-18", 0, 0, 0),
            ("2026-11-19", 1, 1, 1),
            ("2026-11-20", 2, 3, 5),
            ("2026-11-21", 1, 1, 1),
            ("2026-11-22", 0, 0, 0),
        ]
    );
}

#[tokio::test]
async fn timeseries_single_day_and_empty() {
    let db = TestDb::new().await;
    let r = reports::timeseries(&db.ch, scope(P), range("2026-11-20", "2026-11-20"))
        .await
        .unwrap();
    assert_eq!(r.points.len(), 1);
    assert_eq!(r.points[0].date, "2026-11-20");
    assert_eq!(r.points[0].pageviews, 0);
}

#[tokio::test]
async fn timeseries_cross_month_and_tz_boundary() {
    let db = TestDb::new().await;
    insert(
        &db.ch,
        vec![
            ev(P, "page_view", 1, 1, "2026-10-31 23:59:59").start("direct"),
            ev(P, "page_view", 2, 2, "2026-11-01 00:00:00").start("direct"),
        ],
    )
    .await;
    let r = reports::timeseries(&db.ch, scope(P), range("2026-10-31", "2026-11-01"))
        .await
        .unwrap();
    let pv: Vec<u64> = r.points.iter().map(|p| p.pageviews).collect();
    assert_eq!(pv, vec![1, 1]);
}

#[tokio::test]
async fn timeseries_tenant_isolation() {
    let db = TestDb::new().await;
    other_tenant_fixture(&db).await;
    let r = range("2026-11-20", "2026-11-20");
    let p = reports::timeseries(&db.ch, scope(P), r).await.unwrap();
    assert_eq!(p.points[0].visitors, 0);
    let q = reports::timeseries(&db.ch, scope(Q), r).await.unwrap();
    assert_eq!(q.points[0].visitors, 20);
}

// ------------------------------------------------------------------ channels

#[tokio::test]
async fn channels_happy_sorted_desc() {
    let db = TestDb::new().await;
    traffic_fixture(&db, P).await;
    other_tenant_fixture(&db).await;
    let r = range("2026-11-20", "2026-11-21");
    let rows = reports::channels(&db.ch, scope(P), r).await.unwrap();
    let row = |c: &str, s, e, v, pv| ChannelTrafficRow {
        channel: c.into(),
        sessions: s,
        entrances: e,
        visitors: v,
        pageviews: pv,
    };
    assert_eq!(
        rows,
        vec![
            row("naver_sa", 2, 2, 2, 3),
            row("meta", 1, 1, 1, 2),
            row("direct", 1, 1, 1, 1),
        ]
    );
    // 멱등성
    assert_eq!(rows, reports::channels(&db.ch, scope(P), r).await.unwrap());
}

#[tokio::test]
async fn channels_empty_and_tenant_isolation() {
    let db = TestDb::new().await;
    other_tenant_fixture(&db).await;
    let r = range("2026-11-20", "2026-11-20");
    assert!(
        reports::channels(&db.ch, scope(P), r)
            .await
            .unwrap()
            .is_empty()
    );
    let q = reports::channels(&db.ch, scope(Q), r).await.unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].channel, "kakao");
}

// ------------------------------------------------------------------ pages

#[tokio::test]
async fn pages_happy() {
    let db = TestDb::new().await;
    traffic_fixture(&db, P).await;
    other_tenant_fixture(&db).await;
    let rows = reports::pages(
        &db.ch,
        scope(P),
        range("2026-11-20", "2026-11-21"),
        None,
        50,
    )
    .await
    .unwrap();
    let row = |p: &str, pv, v, e| PageRow {
        path: p.into(),
        pageviews: pv,
        visitors: v,
        entrances: e,
    };
    assert_eq!(
        rows,
        vec![
            row("/", 3, 2, 3),
            row("/goods/a", 2, 2, 1),
            row("/cart", 1, 1, 0),
        ]
    );
}

#[tokio::test]
async fn pages_limit_is_capped() {
    let db = TestDb::new().await;
    let evs = (0..(MAX_LIMIT as u128 + 50))
        .map(|i| ev(P, "page_view", i, i, "2026-11-20 10:00:00").path(&format!("/p/{i}")))
        .collect();
    insert(&db.ch, evs).await;
    let r = range("2026-11-20", "2026-11-20");
    let rows = reports::pages(&db.ch, scope(P), r, None, 1_000_000)
        .await
        .unwrap();
    assert_eq!(rows.len(), MAX_LIMIT as usize);
    let rows = reports::pages(&db.ch, scope(P), r, None, 3).await.unwrap();
    assert_eq!(rows.len(), 3);
    // 0은 1로 올림
    let rows = reports::pages(&db.ch, scope(P), r, None, 0).await.unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn pages_path_filter_is_bound_literal() {
    let db = TestDb::new().await;
    let mut evs = vec![
        ev(P, "page_view", 1, 1, "2026-11-20 10:00:00").path("/sale/50%_off"),
        ev(P, "page_view", 2, 2, "2026-11-20 10:00:00").path("/sale/50x_off"),
        ev(P, "page_view", 3, 3, "2026-11-20 10:00:00").path("/q/it's"),
        ev(P, "page_view", 4, 4, "2026-11-20 10:00:00").path("/q/back\\slash"),
        ev(P, "page_view", 5, 5, "2026-11-20 10:00:00").path("/q/'; DROP TABLE events; --"),
    ];
    evs.push(ev(P, "page_view", 6, 6, "2026-11-20 10:00:00").path("/plain"));
    insert(&db.ch, evs).await;
    let r = range("2026-11-20", "2026-11-20");
    let paths = |rows: Vec<PageRow>| rows.into_iter().map(|r| r.path).collect::<Vec<_>>();

    // `%`·`_`는 와일드카드가 아니라 글자 그대로
    let got = reports::pages(&db.ch, scope(P), r, Some("50%_"), 50)
        .await
        .unwrap();
    assert_eq!(paths(got), vec!["/sale/50%_off"]);
    let got = reports::pages(&db.ch, scope(P), r, Some("it's"), 50)
        .await
        .unwrap();
    assert_eq!(paths(got), vec!["/q/it's"]);
    let got = reports::pages(&db.ch, scope(P), r, Some("back\\slash"), 50)
        .await
        .unwrap();
    assert_eq!(paths(got), vec!["/q/back\\slash"]);
    let got = reports::pages(&db.ch, scope(P), r, Some("'; DROP TABLE events; --"), 50)
        .await
        .unwrap();
    assert_eq!(paths(got), vec!["/q/'; DROP TABLE events; --"]);
    for s in NASTY {
        reports::pages(&db.ch, scope(P), r, Some(s), 50)
            .await
            .unwrap();
    }
    // 테이블이 그대로 있어야 한다
    let all = reports::pages(&db.ch, scope(P), r, None, 50).await.unwrap();
    assert_eq!(all.len(), 6);
    // 빈 필터는 필터 없음과 같다
    let empty = reports::pages(&db.ch, scope(P), r, Some(""), 50)
        .await
        .unwrap();
    assert_eq!(empty.len(), 6);
}

#[tokio::test]
async fn pages_tenant_isolation() {
    let db = TestDb::new().await;
    other_tenant_fixture(&db).await;
    let r = range("2026-11-20", "2026-11-20");
    assert!(
        reports::pages(&db.ch, scope(P), r, None, 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        reports::pages(&db.ch, scope(P), r, Some("/q-only"), 50)
            .await
            .unwrap()
            .is_empty()
    );
    let q = reports::pages(&db.ch, scope(Q), r, None, 50).await.unwrap();
    assert_eq!(q[0].path, "/q-only");
    assert_eq!(q[0].pageviews, 20);
}

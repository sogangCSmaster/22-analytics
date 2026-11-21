//! 실제 매출(중복 소거)·중복 조합·주문 상세·최근 주문 리포트.

use crate::common::*;
use analytics_ch::reports::{self, DateRange};
use analytics_core::attribution::Model;
use analytics_core::dto::reports::*;
use analytics_testkit::TestDb;
use std::collections::HashMap;

fn range() -> DateRange {
    DateRange::parse("2026-11-20", "2026-11-21").unwrap()
}

fn no_spend() -> HashMap<String, i64> {
    HashMap::new()
}

/// 매출 픽스처 (lookback 7일, 프로젝트 `project`).
///
/// - ORD-9901 100,000 (11-20 18:45): 18:10 naver_sa → 18:40 meta (매뉴얼 1.A)
/// - ORD-2 30,000 (11-21 10:00): google_ads 한 접점, 부분 환불
/// - ORD-3 20,000 (11-20 12:00): 미귀속
/// - ORD-4 50,000 (11-20 20:00): 취소 → 최신 행이 is_deleted=1
/// - ORD-5 10,000 (11-21 08:00): kakao, 재연산으로 77,777 → 10,000 대체
/// - ORD-OLD (11-10): 기간 밖
/// - ORD-9901 lookback 30: kakao 접점이 추가된 다른 결과 (lookback 7 조회에 섞이면 안 됨)
async fn revenue_fixture(db: &TestDb, project: u32) {
    let a = attr(project, "ORD-9901", "2026-11-20 18:45:00", 7);
    let naver = Some("2026-11-20 18:10:00");
    let meta = Some("2026-11-20 18:40:00");
    let mut rows = Vec::new();
    rows.extend(a.model(
        "last_click",
        &[
            (1, "naver_sa", naver, 0, true),
            (2, "meta", meta, 100_000, false),
        ],
    ));
    rows.extend(a.model(
        "first_click",
        &[
            (1, "naver_sa", naver, 100_000, false),
            (2, "meta", meta, 0, false),
        ],
    ));
    rows.extend(a.model(
        "linear",
        &[
            (1, "naver_sa", naver, 50_000, false),
            (2, "meta", meta, 50_000, false),
        ],
    ));
    rows.extend(a.model(
        "raw_sum",
        &[
            (1, "naver_sa", naver, 100_000, false),
            (2, "meta", meta, 100_000, false),
        ],
    ));

    let a30 = attr(project, "ORD-9901", "2026-11-20 18:45:00", 30);
    let kakao = Some("2026-11-15 09:00:00");
    rows.extend(a30.model(
        "last_click",
        &[
            (1, "kakao", kakao, 0, true),
            (2, "naver_sa", naver, 0, true),
            (3, "meta", meta, 100_000, false),
        ],
    ));
    rows.extend(a30.model(
        "raw_sum",
        &[
            (1, "kakao", kakao, 100_000, false),
            (2, "naver_sa", naver, 100_000, false),
            (3, "meta", meta, 100_000, false),
        ],
    ));

    rows.extend(attr(project, "ORD-2", "2026-11-21 10:00:00", 7).single(
        1,
        "google_ads",
        Some("2026-11-21 09:00:00"),
        30_000,
    ));
    rows.extend(attr(project, "ORD-3", "2026-11-20 12:00:00", 7).single(
        0,
        "(unattributed)",
        None,
        20_000,
    ));
    // 취소: 먼저 계산된 행 → 나중에 삭제 표시
    let ord4 = attr(project, "ORD-4", "2026-11-20 20:00:00", 7).computed("2026-11-20 21:00:00");
    rows.extend(ord4.single(1, "meta", Some("2026-11-20 19:00:00"), 50_000));
    rows.extend(attr(project, "ORD-OLD", "2026-11-10 10:00:00", 7).single(
        1,
        "meta",
        Some("2026-11-10 09:00:00"),
        1_000_000,
    ));
    // 재연산 전 결과
    let ord5_old = attr(project, "ORD-5", "2026-11-21 08:00:00", 7).computed("2026-11-21 08:05:00");
    rows.extend(ord5_old.single(1, "kakao", Some("2026-11-21 07:00:00"), 77_777));
    insert_attr(&db.ch, rows).await;

    // 별도 INSERT로 나중 버전을 쓴다 (ReplacingMergeTree가 병합 전에도 FINAL로 대체해야 함)
    let mut later = Vec::new();
    let ord4_del = attr(project, "ORD-4", "2026-11-20 20:00:00", 7)
        .computed("2026-11-21 09:00:00")
        .deleted();
    later.extend(ord4_del.single(1, "meta", Some("2026-11-20 19:00:00"), 50_000));
    let ord5 = attr(project, "ORD-5", "2026-11-21 08:00:00", 7).computed("2026-11-22 04:00:00");
    later.extend(ord5.single(1, "kakao", Some("2026-11-21 07:00:00"), 10_000));
    insert_attr(&db.ch, later).await;

    insert_orders(
        &db.ch,
        vec![
            order(
                project,
                "ORD-9901",
                "CONFIRMED",
                100_000,
                0,
                "2026-11-20 18:45:00",
                1,
            ),
            order(
                project,
                "ORD-2",
                "PARTIALLY_REFUNDED",
                40_000,
                10_000,
                "2026-11-21 10:00:00",
                2,
            ),
            order(
                project,
                "ORD-3",
                "CONFIRMED",
                20_000,
                0,
                "2026-11-20 12:00:00",
                1,
            ),
            order(
                project,
                "ORD-4",
                "CANCELLED",
                50_000,
                50_000,
                "2026-11-20 20:00:00",
                2,
            ),
            order(
                project,
                "ORD-5",
                "CONFIRMED",
                10_000,
                0,
                "2026-11-21 08:00:00",
                1,
            ),
            order(
                project,
                "ORD-OLD",
                "CONFIRMED",
                1_000_000,
                0,
                "2026-11-10 10:00:00",
                1,
            ),
        ],
    )
    .await;
}

fn by_channel(r: &RevenueReport) -> Vec<(&str, i64, i64, u64)> {
    r.channels
        .iter()
        .map(|c| (c.channel.as_str(), c.raw_claimed, c.attributed, c.assists))
        .collect()
}

// ------------------------------------------------------------------ revenue_report

#[tokio::test]
async fn revenue_last_click_happy() {
    let db = TestDb::new().await;
    revenue_fixture(&db, P).await;
    revenue_fixture(&db, Q).await;
    let spend: HashMap<String, i64> = [
        ("meta".to_string(), 50_000),
        ("naver_sa".to_string(), 20_000),
        ("kakao".to_string(), 0),
        ("tiktok".to_string(), 5_000),
    ]
    .into();
    let r = reports::revenue_report(&db.ch, scope(P), range(), Model::LastClick, 7, &spend)
        .await
        .unwrap();
    assert_eq!(r.model, Model::LastClick);
    assert_eq!(r.lookback_days, 7);
    assert_eq!(r.raw_sum, 260_000);
    assert_eq!(r.deduplicated, 160_000);
    assert_eq!(r.duplicate_amount, 100_000);
    assert!((r.duplicate_rate - 100_000.0 / 260_000.0).abs() < 1e-12);
    assert_eq!(r.orders, 4);
    assert_eq!(r.spend_total, 75_000);
    assert!((r.roas.unwrap() - 160_000.0 / 75_000.0).abs() < 1e-12);
    assert_eq!(
        by_channel(&r),
        vec![
            ("meta", 100_000, 100_000, 0),
            ("google_ads", 30_000, 30_000, 0),
            ("(unattributed)", 20_000, 20_000, 0),
            ("kakao", 10_000, 10_000, 0),
            ("naver_sa", 100_000, 0, 1),
            ("tiktok", 0, 0, 0),
        ]
    );
    let get = |c: &str| r.channels.iter().find(|x| x.channel == c).unwrap();
    assert_eq!(get("meta").share, 0.625);
    assert_eq!(get("meta").spend, 50_000);
    assert_eq!(get("meta").roas, Some(2.0));
    assert_eq!(get("naver_sa").roas, Some(0.0));
    // 광고비 0원 / 없음 → ROAS 없음
    assert_eq!(get("kakao").roas, None);
    assert_eq!(get("google_ads").spend, 0);
    assert_eq!(get("google_ads").roas, None);
    assert_eq!(get("tiktok").spend, 5_000);
    assert_eq!(get("tiktok").roas, Some(0.0));
    assert_eq!(get("tiktok").share, 0.0);
}

#[tokio::test]
async fn revenue_manual_scenario_linear_and_first_click() {
    let db = TestDb::new().await;
    revenue_fixture(&db, P).await;
    let r = reports::revenue_report(&db.ch, scope(P), range(), Model::Linear, 7, &no_spend())
        .await
        .unwrap();
    assert_eq!(r.deduplicated, 160_000);
    assert_eq!(
        by_channel(&r),
        vec![
            ("meta", 100_000, 50_000, 0),
            ("naver_sa", 100_000, 50_000, 1),
            ("google_ads", 30_000, 30_000, 0),
            ("(unattributed)", 20_000, 20_000, 0),
            ("kakao", 10_000, 10_000, 0),
        ]
    );
    assert_eq!(r.roas, None);
    let f = reports::revenue_report(&db.ch, scope(P), range(), Model::FirstClick, 7, &no_spend())
        .await
        .unwrap();
    assert_eq!(f.channels[0].channel, "naver_sa");
    assert_eq!(f.channels[0].attributed, 100_000);

    // 매뉴얼 1.A 단독: 11-20 18:45 하루만 보면 ORD-9901 + ORD-3(미귀속) + ORD-4(취소)
    let one = DateRange::parse("2026-11-20", "2026-11-20").unwrap();
    let r = reports::revenue_report(&db.ch, scope(P), one, Model::LastClick, 7, &no_spend())
        .await
        .unwrap();
    assert_eq!((r.raw_sum, r.deduplicated, r.orders), (220_000, 120_000, 2));
}

#[tokio::test]
async fn revenue_lookback_is_separate() {
    let db = TestDb::new().await;
    revenue_fixture(&db, P).await;
    let r = reports::revenue_report(&db.ch, scope(P), range(), Model::LastClick, 30, &no_spend())
        .await
        .unwrap();
    assert_eq!(r.raw_sum, 300_000);
    assert_eq!(r.deduplicated, 100_000);
    assert_eq!(r.orders, 1);
    assert_eq!(r.duplicate_amount, 200_000);
    let kakao = r.channels.iter().find(|c| c.channel == "kakao").unwrap();
    assert_eq!(
        (kakao.raw_claimed, kakao.attributed, kakao.assists),
        (100_000, 0, 1)
    );
}

#[tokio::test]
async fn revenue_is_idempotent_and_ignores_superseded_rows() {
    let db = TestDb::new().await;
    revenue_fixture(&db, P).await;
    let a = reports::revenue_report(&db.ch, scope(P), range(), Model::LastClick, 7, &no_spend())
        .await
        .unwrap();
    // 같은 연산 결과를 한 번 더 적재 (잡 재실행): 결과가 같아야 한다
    let again = attr(P, "ORD-5", "2026-11-21 08:00:00", 7).computed("2026-11-22 04:00:00");
    insert_attr(
        &db.ch,
        again.single(1, "kakao", Some("2026-11-21 07:00:00"), 10_000),
    )
    .await;
    let b = reports::revenue_report(&db.ch, scope(P), range(), Model::LastClick, 7, &no_spend())
        .await
        .unwrap();
    assert_eq!(a, b);
    let kakao = b.channels.iter().find(|c| c.channel == "kakao").unwrap();
    assert_eq!(
        kakao.attributed, 10_000,
        "재연산 전 77,777은 대체되어야 한다"
    );
    assert!(
        b.channels.iter().all(|c| c.attributed != 50_000),
        "취소 주문 제외"
    );
}

#[tokio::test]
async fn revenue_empty_project_is_zero() {
    let db = TestDb::new().await;
    let spend: HashMap<String, i64> = [("meta".to_string(), 0)].into();
    let r = reports::revenue_report(&db.ch, scope(P), range(), Model::LastClick, 7, &spend)
        .await
        .unwrap();
    assert_eq!(
        (r.raw_sum, r.deduplicated, r.duplicate_amount, r.orders),
        (0, 0, 0, 0)
    );
    assert_eq!(r.duplicate_rate, 0.0);
    assert_eq!(r.roas, None);
    assert_eq!(r.channels.len(), 1);
    assert_eq!(r.channels[0].share, 0.0);
    assert_eq!(r.channels[0].roas, None);
}

#[tokio::test]
async fn revenue_tenant_isolation() {
    let db = TestDb::new().await;
    revenue_fixture(&db, Q).await;
    let r = reports::revenue_report(&db.ch, scope(P), range(), Model::LastClick, 7, &no_spend())
        .await
        .unwrap();
    assert_eq!((r.raw_sum, r.deduplicated, r.orders), (0, 0, 0));
    assert!(r.channels.is_empty());
    let q = reports::revenue_report(&db.ch, scope(Q), range(), Model::LastClick, 7, &no_spend())
        .await
        .unwrap();
    assert_eq!(q.deduplicated, 160_000);
}

// ------------------------------------------------------------------ overlap_paths

async fn overlap_fixture(db: &TestDb, project: u32) {
    let t = Some("2026-11-20 09:00:00");
    let mut rows = Vec::new();
    let mut lc = |id: &str, at: &str, chs: &[&str]| {
        let a = attr(project, id, at, 7);
        let n = chs.len();
        let touches: Vec<(u16, &str, Option<&str>, i64, bool)> = chs
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let last = i + 1 == n;
                ((i + 1) as u16, *c, t, if last { 1_000 } else { 0 }, !last)
            })
            .collect();
        rows.extend(a.model("last_click", &touches));
    };
    lc("O1", "2026-11-20 10:00:00", &["naver_sa", "meta"]);
    lc("O2", "2026-11-20 11:00:00", &["naver_sa", "meta"]);
    // 연속 중복은 합침 → naver_sa → meta
    lc(
        "O3",
        "2026-11-21 11:00:00",
        &["naver_sa", "naver_sa", "meta"],
    );
    lc("O4", "2026-11-20 12:00:00", &["meta", "naver_sa"]);
    lc(
        "O5",
        "2026-11-20 13:00:00",
        &["naver_sa", "meta", "naver_sa"],
    );
    // 채널 하나뿐 → 제외
    lc("O6", "2026-11-20 14:00:00", &["kakao", "kakao"]);
    lc("O7", "2026-11-20 15:00:00", &["kakao"]);
    // 기간 밖
    lc("O8", "2026-11-25 10:00:00", &["meta", "kakao"]);
    insert_attr(&db.ch, rows).await;
    // 취소된 주문 (삭제 표시) → 제외
    let del = attr(project, "O9", "2026-11-20 16:00:00", 7).deleted();
    insert_attr(
        &db.ch,
        del.model(
            "last_click",
            &[(1, "google_ads", t, 0, true), (2, "meta", t, 1_000, false)],
        ),
    )
    .await;
}

#[tokio::test]
async fn overlap_paths_happy() {
    let db = TestDb::new().await;
    overlap_fixture(&db, P).await;
    overlap_fixture(&db, Q).await;
    let r = reports::overlap_paths(&db.ch, scope(P), range(), 7, 50)
        .await
        .unwrap();
    assert_eq!(r.multi_channel_orders, 5);
    let got: Vec<(Vec<&str>, u64)> = r
        .paths
        .iter()
        .map(|p| (p.channels.iter().map(String::as_str).collect(), p.orders))
        .collect();
    assert_eq!(
        got,
        vec![
            (vec!["naver_sa", "meta"], 3),
            (vec!["meta", "naver_sa"], 1),
            (vec!["naver_sa", "meta", "naver_sa"], 1),
        ]
    );
    // 상위 N 제한은 경로 수만 줄이고 전체 주문 수는 유지
    let top1 = reports::overlap_paths(&db.ch, scope(P), range(), 7, 1)
        .await
        .unwrap();
    assert_eq!(top1.paths.len(), 1);
    assert_eq!(top1.multi_channel_orders, 5);
    assert_eq!(
        r,
        reports::overlap_paths(&db.ch, scope(P), range(), 7, 50)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn overlap_paths_empty_and_tenant() {
    let db = TestDb::new().await;
    overlap_fixture(&db, Q).await;
    let r = reports::overlap_paths(&db.ch, scope(P), range(), 7, 50)
        .await
        .unwrap();
    assert_eq!(r.multi_channel_orders, 0);
    assert!(r.paths.is_empty());
    let other_lookback = reports::overlap_paths(&db.ch, scope(Q), range(), 30, 50)
        .await
        .unwrap();
    assert!(other_lookback.paths.is_empty());
}

// ------------------------------------------------------------------ order_detail

#[tokio::test]
async fn order_detail_happy() {
    let db = TestDb::new().await;
    revenue_fixture(&db, P).await;
    let d = reports::order_detail(&db.ch, scope(P), "ORD-9901", 7)
        .await
        .unwrap()
        .expect("주문 있음");
    assert_eq!(d.order.order_id, "ORD-9901");
    assert_eq!(d.order.status, "CONFIRMED");
    assert_eq!(d.order.effective_amount, 100_000);
    assert_eq!(d.order.ordered_at, "2026-11-20T18:45:00+09:00");
    assert_eq!(d.lookback_days, 7);
    assert_eq!(
        d.touches,
        vec![
            OrderTouch {
                touch_index: 1,
                ts: "2026-11-20T18:10:00+09:00".into(),
                channel: "naver_sa".into(),
                campaign: "cmp_naver_sa".into(),
            },
            OrderTouch {
                touch_index: 2,
                ts: "2026-11-20T18:40:00+09:00".into(),
                channel: "meta".into(),
                campaign: "cmp_meta".into(),
            },
        ]
    );
    let models: Vec<Model> = d.shares.iter().map(|m| m.model).collect();
    assert_eq!(
        models,
        vec![
            Model::LastClick,
            Model::FirstClick,
            Model::Linear,
            Model::RawSum
        ]
    );
    let linear: Vec<(&str, i64)> = d.shares[2]
        .rows
        .iter()
        .map(|r| (r.channel.as_str(), r.amount))
        .collect();
    assert_eq!(linear, vec![("naver_sa", 50_000), ("meta", 50_000)]);
    assert!(d.shares[0].rows[0].is_assist);

    // lookback 30은 다른 접점
    let d30 = reports::order_detail(&db.ch, scope(P), "ORD-9901", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d30.touches.len(), 3);
}

#[tokio::test]
async fn order_detail_edge_cases() {
    let db = TestDb::new().await;
    revenue_fixture(&db, P).await;
    // 없는 주문
    assert!(
        reports::order_detail(&db.ch, scope(P), "NOPE", 7)
            .await
            .unwrap()
            .is_none()
    );
    // 미귀속 주문: 접점 없음, 배분은 touch_index 0
    let d = reports::order_detail(&db.ch, scope(P), "ORD-3", 7)
        .await
        .unwrap()
        .unwrap();
    assert!(d.touches.is_empty());
    assert_eq!(d.shares[0].rows[0].touch_index, 0);
    assert_eq!(d.shares[0].rows[0].channel, "(unattributed)");
    // 취소 주문: 주문 정보는 보이고 배분은 없다
    let d = reports::order_detail(&db.ch, scope(P), "ORD-4", 7)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.order.status, "CANCELLED");
    assert_eq!(d.order.effective_amount, 0);
    assert!(d.shares.is_empty());
    assert!(d.touches.is_empty());
    // 재연산된 주문은 최신 값
    let d = reports::order_detail(&db.ch, scope(P), "ORD-5", 7)
        .await
        .unwrap()
        .unwrap();
    assert!(d.shares.iter().all(|m| m.rows[0].amount == 10_000));
    // 인젝션 시도
    for s in NASTY {
        assert!(
            reports::order_detail(&db.ch, scope(P), s, 7)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn order_detail_tenant_isolation() {
    let db = TestDb::new().await;
    revenue_fixture(&db, Q).await;
    assert!(
        reports::order_detail(&db.ch, scope(P), "ORD-9901", 7)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        reports::order_detail(&db.ch, scope(Q), "ORD-9901", 7)
            .await
            .unwrap()
            .is_some()
    );
}

// ------------------------------------------------------------------ recent_orders

#[tokio::test]
async fn recent_orders_happy_and_paging() {
    let db = TestDb::new().await;
    revenue_fixture(&db, P).await;
    revenue_fixture(&db, Q).await;
    let r = reports::recent_orders(&db.ch, scope(P), range(), 7, 10, 0)
        .await
        .unwrap();
    assert_eq!(r.total, 5);
    let got: Vec<(&str, &str, i64, &str, Option<&str>)> = r
        .rows
        .iter()
        .map(|o| {
            (
                o.order_id.as_str(),
                o.ordered_at.as_str(),
                o.effective_amount,
                o.status.as_str(),
                o.last_click_channel.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "ORD-2",
                "2026-11-21T10:00:00+09:00",
                30_000,
                "PARTIALLY_REFUNDED",
                Some("google_ads")
            ),
            (
                "ORD-5",
                "2026-11-21T08:00:00+09:00",
                10_000,
                "CONFIRMED",
                Some("kakao")
            ),
            ("ORD-4", "2026-11-20T20:00:00+09:00", 0, "CANCELLED", None),
            (
                "ORD-9901",
                "2026-11-20T18:45:00+09:00",
                100_000,
                "CONFIRMED",
                Some("meta")
            ),
            (
                "ORD-3",
                "2026-11-20T12:00:00+09:00",
                20_000,
                "CONFIRMED",
                Some("(unattributed)")
            ),
        ]
    );
    let page = reports::recent_orders(&db.ch, scope(P), range(), 7, 2, 1)
        .await
        .unwrap();
    assert_eq!(page.total, 5);
    let ids: Vec<&str> = page.rows.iter().map(|o| o.order_id.as_str()).collect();
    assert_eq!(ids, vec!["ORD-5", "ORD-4"]);
    // 범위를 넘는 offset
    let past = reports::recent_orders(&db.ch, scope(P), range(), 7, 10, 100)
        .await
        .unwrap();
    assert!(past.rows.is_empty());
    assert_eq!(past.total, 5);
}

#[tokio::test]
async fn recent_orders_empty_and_tenant() {
    let db = TestDb::new().await;
    revenue_fixture(&db, Q).await;
    let r = reports::recent_orders(&db.ch, scope(P), range(), 7, 10, 0)
        .await
        .unwrap();
    assert_eq!(r.total, 0);
    assert!(r.rows.is_empty());
    let q = reports::recent_orders(&db.ch, scope(Q), range(), 7, 10, 0)
        .await
        .unwrap();
    assert_eq!(q.total, 5);
}

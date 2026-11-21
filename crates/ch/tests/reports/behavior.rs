//! 이벤트·속성·실시간·전자상거래 퍼널 리포트.

use crate::common::*;
use analytics_ch::reports::{self, DateRange, FUNNEL_STEPS, MAX_LIMIT, REALTIME_TOP_PAGES};
use analytics_core::dto::reports::*;
use analytics_testkit::TestDb;
use time::Duration;

fn day(d: &str) -> DateRange {
    DateRange::parse(d, d).unwrap()
}

// ------------------------------------------------------------------ events

async fn events_fixture(db: &TestDb, project: u32) {
    insert(
        &db.ch,
        vec![
            ev(project, "page_view", 1, 1, "2026-11-20 10:00:00"),
            ev(project, "page_view", 1, 1, "2026-11-20 10:01:00"),
            ev(project, "page_view", 2, 2, "2026-11-20 10:02:00"),
            ev(project, "signup", 1, 1, "2026-11-20 10:03:00").prop("plan", "pro"),
            ev(project, "signup", 2, 2, "2026-11-20 10:04:00").prop("plan", "free"),
            ev(project, "signup", 3, 3, "2026-11-20 10:05:00").prop("plan", "pro"),
            ev(project, "signup", 3, 3, "2026-11-20 10:06:00")
                .prop("plan", "pro")
                .prop("ref", "x"),
            ev(project, "signup", 4, 4, "2026-11-20 10:07:00").prop("ref", "y"),
            // 다음 날 (KST 00:00:00) → 제외
            ev(project, "signup", 5, 5, "2026-11-21 00:00:00").prop("plan", "pro"),
            // 전날 23:59:59 → 제외
            ev(project, "signup", 6, 6, "2026-11-19 23:59:59").prop("plan", "pro"),
        ],
    )
    .await;
}

#[tokio::test]
async fn events_happy() {
    let db = TestDb::new().await;
    events_fixture(&db, P).await;
    insert(&db.ch, vec![ev(Q, "q_secret", 9, 9, "2026-11-20 10:00:00")]).await;
    let rows = reports::events(&db.ch, scope(P), day("2026-11-20"), 50)
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![
            EventCountRow {
                event_name: "signup".into(),
                count: 5,
                visitors: 4
            },
            EventCountRow {
                event_name: "page_view".into(),
                count: 3,
                visitors: 2
            },
        ]
    );
    let again = reports::events(&db.ch, scope(P), day("2026-11-20"), 50)
        .await
        .unwrap();
    assert_eq!(rows, again);
}

#[tokio::test]
async fn events_empty_limit_and_tenant() {
    let db = TestDb::new().await;
    insert(&db.ch, vec![ev(Q, "q_secret", 9, 9, "2026-11-20 10:00:00")]).await;
    assert!(
        reports::events(&db.ch, scope(P), day("2026-11-20"), 50)
            .await
            .unwrap()
            .is_empty()
    );
    let q = reports::events(&db.ch, scope(Q), day("2026-11-20"), 50)
        .await
        .unwrap();
    assert_eq!(q[0].event_name, "q_secret");

    let evs = (0..(MAX_LIMIT as u128 + 10))
        .map(|i| ev(P, &format!("e_{i}"), i, i, "2026-11-20 10:00:00"))
        .collect();
    insert(&db.ch, evs).await;
    let rows = reports::events(&db.ch, scope(P), day("2026-11-20"), u32::MAX)
        .await
        .unwrap();
    assert_eq!(rows.len(), MAX_LIMIT as usize);
}

#[tokio::test]
async fn event_properties_happy() {
    let db = TestDb::new().await;
    events_fixture(&db, P).await;
    let rows = reports::event_properties(&db.ch, scope(P), day("2026-11-20"), "signup", "plan", 50)
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![
            PropertyValueRow {
                value: "pro".into(),
                count: 3,
                visitors: 2
            },
            PropertyValueRow {
                value: "free".into(),
                count: 1,
                visitors: 1
            },
        ]
    );
    // 속성이 없는 키 / 없는 이벤트 → 빈 목록
    assert!(
        reports::event_properties(&db.ch, scope(P), day("2026-11-20"), "signup", "nope", 50)
            .await
            .unwrap()
            .is_empty()
    );
    let limited =
        reports::event_properties(&db.ch, scope(P), day("2026-11-20"), "signup", "plan", 1)
            .await
            .unwrap();
    assert_eq!(limited.len(), 1);
}

#[tokio::test]
async fn event_properties_tenant_isolation() {
    let db = TestDb::new().await;
    events_fixture(&db, Q).await;
    assert!(
        reports::event_properties(&db.ch, scope(P), day("2026-11-20"), "signup", "plan", 50)
            .await
            .unwrap()
            .is_empty()
    );
    let q = reports::event_properties(&db.ch, scope(Q), day("2026-11-20"), "signup", "plan", 50)
        .await
        .unwrap();
    assert_eq!(q.len(), 2);
}

#[tokio::test]
async fn event_properties_injection_is_bound() {
    let db = TestDb::new().await;
    let mut evs = Vec::new();
    for (i, s) in NASTY.iter().enumerate() {
        // 이벤트 이름 규칙상 특수문자가 올 수 없지만, 조회 계층은 어떤 입력도 바인딩으로 처리해야 한다.
        evs.push(ev(P, s, i as u128, i as u128, "2026-11-20 10:00:00").prop(s, s));
        evs.push(
            ev(P, "custom", i as u128, i as u128, "2026-11-20 10:00:00").prop(s, &format!("v{i}")),
        );
    }
    insert(&db.ch, evs).await;
    for (i, s) in NASTY.iter().enumerate() {
        let rows = reports::event_properties(&db.ch, scope(P), day("2026-11-20"), s, s, 50)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "{s}");
        assert_eq!(rows[0].value, *s);
        let rows = reports::event_properties(&db.ch, scope(P), day("2026-11-20"), "custom", s, 50)
            .await
            .unwrap();
        assert_eq!(rows[0].value, format!("v{i}"), "{s}");
    }
    // `%` 키가 다른 키에 와일드카드로 매칭되지 않는다
    assert!(
        reports::event_properties(&db.ch, scope(P), day("2026-11-20"), "custom", "%", 50)
            .await
            .unwrap()
            .is_empty()
    );
    // 테이블이 남아 있다
    let all = reports::events(&db.ch, scope(P), day("2026-11-20"), 50)
        .await
        .unwrap();
    assert_eq!(all.len(), NASTY.len() + 1);
}

// ------------------------------------------------------------------ realtime

#[tokio::test]
async fn realtime_happy() {
    let db = TestDb::new().await;
    let now = kst("2026-11-21 15:00:30");
    let at = |d: i64| now - Duration::seconds(d);
    insert(
        &db.ch,
        vec![
            ev_at(P, "page_view", 1, 1, at(60)).path("/a"),
            ev_at(P, "page_view", 2, 2, at(600)).path("/b"),
            ev_at(P, "click", 3, 3, at(120)).path("/a"),
            // 30분 밖
            ev_at(P, "page_view", 4, 4, at(40 * 60)).path("/old"),
            // 미래
            ev_at(P, "page_view", 5, 5, now + Duration::seconds(60)).path("/future"),
            // 다른 테넌트
            ev_at(Q, "page_view", 6, 6, at(30)).path("/q"),
        ],
    )
    .await;
    let r = reports::realtime(&db.ch, scope(P), now).await.unwrap();
    assert_eq!(r.active_5m, 2);
    assert_eq!(r.active_30m, 3);
    assert_eq!(r.per_minute.len(), 30);
    assert_eq!(r.per_minute[0].minute, "14:31");
    assert_eq!(r.per_minute[29].minute, "15:00");
    let nonzero: Vec<(&str, u64)> = r
        .per_minute
        .iter()
        .filter(|m| m.pageviews > 0)
        .map(|m| (m.minute.as_str(), m.pageviews))
        .collect();
    assert_eq!(nonzero, vec![("14:50", 1), ("14:59", 1)]);
    assert_eq!(
        r.top_pages,
        vec![
            RealtimePage {
                path: "/a".into(),
                pageviews: 1
            },
            RealtimePage {
                path: "/b".into(),
                pageviews: 1
            },
        ]
    );
    assert_eq!(r.now, "2026-11-21T15:00:30+09:00");
    assert_eq!(r, reports::realtime(&db.ch, scope(P), now).await.unwrap());
}

#[tokio::test]
async fn realtime_window_boundaries() {
    let db = TestDb::new().await;
    let now = kst("2026-11-21 00:00:00");
    insert(
        &db.ch,
        vec![
            // 정확히 5분 전: 5분 창 밖, 30분 창 안
            ev_at(P, "page_view", 1, 1, now - Duration::minutes(5)),
            // 4분 59초 전: 5분 창 안
            ev_at(P, "page_view", 2, 2, now - Duration::seconds(299)),
            // 정확히 30분 전: 30분 창 밖
            ev_at(P, "page_view", 3, 3, now - Duration::minutes(30)),
            // 지금 시각: 포함
            ev_at(P, "page_view", 4, 4, now),
        ],
    )
    .await;
    let r = reports::realtime(&db.ch, scope(P), now).await.unwrap();
    assert_eq!(r.active_5m, 2);
    assert_eq!(r.active_30m, 3);
    // 날짜 경계를 넘는 분 라벨
    assert_eq!(r.per_minute[0].minute, "23:31");
    assert_eq!(r.per_minute[29].minute, "00:00");
    assert_eq!(r.per_minute[29].pageviews, 1);
}

/// 일자 조건(정렬키 프루닝)을 넣어도 KST 자정·월 경계를 넘는 창이 그대로 집계된다.
#[tokio::test]
async fn realtime_window_crossing_midnight_and_month() {
    let db = TestDb::new().await;
    let now = kst("2026-12-01 00:10:00");
    insert(
        &db.ch,
        vec![
            // 전날(11-30) 23:45 · 23:58
            ev_at(P, "page_view", 1, 1, now - Duration::minutes(25)).path("/before"),
            ev_at(P, "page_view", 2, 2, now - Duration::minutes(12)).path("/before"),
            // 자정 이후 00:08
            ev_at(P, "page_view", 3, 3, now - Duration::minutes(2)).path("/after"),
            // 하루 전 같은 시각: 창 밖
            ev_at(P, "page_view", 4, 4, now - Duration::days(1)).path("/yesterday"),
        ],
    )
    .await;
    let r = reports::realtime(&db.ch, scope(P), now).await.unwrap();
    assert_eq!((r.active_5m, r.active_30m), (1, 3));
    let nonzero: Vec<(&str, u64)> = r
        .per_minute
        .iter()
        .filter(|m| m.pageviews > 0)
        .map(|m| (m.minute.as_str(), m.pageviews))
        .collect();
    assert_eq!(nonzero, vec![("23:45", 1), ("23:58", 1), ("00:08", 1)]);
    assert_eq!(
        r.top_pages,
        vec![
            RealtimePage {
                path: "/before".into(),
                pageviews: 2
            },
            RealtimePage {
                path: "/after".into(),
                pageviews: 1
            },
        ]
    );
}

#[tokio::test]
async fn realtime_empty_and_top_pages_cap() {
    let db = TestDb::new().await;
    let now = kst("2026-11-21 15:00:00");
    let r = reports::realtime(&db.ch, scope(P), now).await.unwrap();
    assert_eq!((r.active_5m, r.active_30m), (0, 0));
    assert_eq!(r.per_minute.len(), 30);
    assert!(r.per_minute.iter().all(|m| m.pageviews == 0));
    assert!(r.top_pages.is_empty());

    let evs = (0..30u128)
        .map(|i| ev_at(P, "page_view", i, i, now - Duration::seconds(10)).path(&format!("/p{i}")))
        .collect();
    insert(&db.ch, evs).await;
    let r = reports::realtime(&db.ch, scope(P), now).await.unwrap();
    assert_eq!(r.top_pages.len(), REALTIME_TOP_PAGES as usize);
    // 다른 테넌트에서는 보이지 않는다
    let q = reports::realtime(&db.ch, scope(Q), now).await.unwrap();
    assert_eq!(q.active_30m, 0);
    assert!(q.top_pages.is_empty());
}

// ------------------------------------------------------------------ funnel

async fn funnel_fixture(db: &TestDb, project: u32) {
    let e = |name: &str, v: u128, t: &str| ev(project, name, v, v, t);
    insert(
        &db.ch,
        vec![
            // v1: 전 단계 완료
            e("view_item", 1, "2026-11-20 10:00:00"),
            e("add_to_cart", 1, "2026-11-20 10:01:00"),
            e("begin_checkout", 1, "2026-11-20 10:02:00"),
            e("purchase", 1, "2026-11-20 10:03:00"),
            // v2: 장바구니까지
            e("view_item", 2, "2026-11-20 11:00:00"),
            e("add_to_cart", 2, "2026-11-20 11:01:00"),
            // v3: 상품 조회 없이 장바구니 → 0단계
            e("add_to_cart", 3, "2026-11-20 12:00:00"),
            // v4: 순서가 뒤바뀜 (구매 후 조회) → 1단계
            e("purchase", 4, "2026-11-20 09:00:00"),
            e("view_item", 4, "2026-11-20 09:10:00"),
            // v5: 조회만
            e("view_item", 5, "2026-11-20 13:00:00"),
            // v6: 다른 날 → 제외
            e("view_item", 6, "2026-11-21 00:00:00"),
        ],
    )
    .await;
}

#[tokio::test]
async fn funnel_happy() {
    let db = TestDb::new().await;
    funnel_fixture(&db, P).await;
    let r = reports::ecommerce_funnel(&db.ch, scope(P), day("2026-11-20"))
        .await
        .unwrap();
    let names: Vec<&str> = r.steps.iter().map(|s| s.event_name.as_str()).collect();
    assert_eq!(names, FUNNEL_STEPS.to_vec());
    let counts: Vec<u64> = r.steps.iter().map(|s| s.visitors).collect();
    assert_eq!(counts, vec![4, 2, 1, 1]);
    let prev: Vec<f64> = r.steps.iter().map(|s| s.rate_from_prev).collect();
    assert_eq!(prev, vec![1.0, 0.5, 0.5, 1.0]);
    let first: Vec<f64> = r.steps.iter().map(|s| s.rate_from_first).collect();
    assert_eq!(first, vec![1.0, 0.5, 0.25, 0.25]);
    assert_eq!(
        r,
        reports::ecommerce_funnel(&db.ch, scope(P), day("2026-11-20"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn funnel_empty_has_zero_rates() {
    let db = TestDb::new().await;
    funnel_fixture(&db, Q).await;
    let r = reports::ecommerce_funnel(&db.ch, scope(P), day("2026-11-20"))
        .await
        .unwrap();
    assert_eq!(r.steps.len(), 4);
    for s in &r.steps {
        assert_eq!(s.visitors, 0);
        assert_eq!(s.rate_from_prev, 0.0);
        assert_eq!(s.rate_from_first, 0.0);
        assert!(s.rate_from_prev.is_finite());
    }
    // 다른 테넌트에는 데이터가 있다
    let q = reports::ecommerce_funnel(&db.ch, scope(Q), day("2026-11-20"))
        .await
        .unwrap();
    assert_eq!(q.steps[0].visitors, 4);
}

//! 기여 연산 잡 통합 테스트 (격리된 Postgres·ClickHouse).
//!
//! 주문·터치포인트·identity 행은 다른 패키지가 쓰는 테이블이라 픽스처로 직접 넣는다.

use analytics_ch::attribution::fixtures::*;
use analytics_core::attribution::DAY_MS;
use analytics_jobs::attribution::{self as job, Mode, worker};
use analytics_pg::attribution_settings as settings;
use analytics_testkit::TestDb;
use clickhouse::Row;
use serde::Deserialize;
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

const MIN: i64 = 60_000;
/// 2026-11-20 18:45 KST (09:45 UTC): 매뉴얼 1.A ORD-9901 결제 시각
const ORDER_AT: i64 = 1_795_167_900_000;
const NOW: i64 = ORDER_AT + 60 * MIN;

#[derive(Debug, Clone, PartialEq, Eq, Row, Deserialize)]
struct Final {
    model: String,
    lookback_days: u8,
    order_id: String,
    touch_index: u16,
    channel: String,
    utm_campaign: String,
    amount: i64,
    is_assist: u8,
}

async fn project(pool: &PgPool) -> u32 {
    project_with_status(pool, "active").await
}

async fn project_with_status(pool: &PgPool, status: &str) -> u32 {
    let org: i64 =
        sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('광고주') RETURNING id")
            .fetch_one(pool)
            .await
            .unwrap();
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO projects (org_id, name, public_key, salt, status) \
         VALUES ($1, 'p', $2, 'salt', $3) RETURNING id",
    )
    .bind(org)
    .bind(analytics_pg::new_public_key())
    .bind(status)
    .fetch_one(pool)
    .await
    .unwrap();
    id as u32
}

async fn finals(db: &TestDb, p: u32) -> Vec<Final> {
    db.ch
        .query(
            "SELECT model, lookback_days, order_id, touch_index, channel, utm_campaign, amount, is_assist \
             FROM attributed_revenue FINAL WHERE project_id = ? AND is_deleted = 0 \
             ORDER BY order_id, model, lookback_days, touch_index",
        )
        .bind(p)
        .fetch_all()
        .await
        .unwrap()
}

async fn raw_count(db: &TestDb, p: u32) -> u64 {
    db.ch
        .query("SELECT count() FROM attributed_revenue WHERE project_id = ?")
        .bind(p)
        .fetch_one()
        .await
        .unwrap()
}

fn pick(rows: &[Final], order: &str, model: &str, lb: u8) -> Vec<(String, i64)> {
    rows.iter()
        .filter(|r| r.order_id == order && r.model == model && r.lookback_days == lb)
        .map(|r| (r.channel.clone(), r.amount))
        .collect()
}

fn total(rows: &[Final], order: &str, model: &str, lb: u8) -> i64 {
    pick(rows, order, model, lb).iter().map(|x| x.1).sum()
}

/// 매뉴얼 1.A: 18:10 네이버 검색광고 → 18:40 메타 → 18:45 ORD-9901 100,000원.
async fn manual_scenario(db: &TestDb, p: u32, visitor: Uuid) {
    insert_touches(
        &db.ch,
        &[
            TouchFixture::utm(p, visitor, ORDER_AT - 35 * MIN, "naver", "cpc", "brand"),
            TouchFixture::utm(
                p,
                visitor,
                ORDER_AT - 5 * MIN,
                "facebook",
                "paid",
                "retarget",
            ),
        ],
    )
    .await
    .unwrap();
    insert_orders(
        &db.ch,
        &[OrderFixture::confirmed(
            p,
            "ORD-9901",
            Some(visitor),
            100_000,
            ORDER_AT,
            1_000,
        )],
    )
    .await
    .unwrap();
}

fn assert_manual(rows: &[Final], amount: i64) {
    for lb in [7, 30] {
        assert_eq!(
            pick(rows, "ORD-9901", "last_click", lb),
            vec![("naver_sa".into(), 0), ("meta".into(), amount)],
            "lb={lb}"
        );
        assert_eq!(
            pick(rows, "ORD-9901", "first_click", lb),
            vec![("naver_sa".into(), amount), ("meta".into(), 0)]
        );
        assert_eq!(
            pick(rows, "ORD-9901", "linear", lb),
            vec![("naver_sa".into(), amount / 2), ("meta".into(), amount / 2)]
        );
        assert_eq!(total(rows, "ORD-9901", "time_decay", lb), amount);
        assert_eq!(total(rows, "ORD-9901", "raw_sum", lb), 2 * amount);
    }
}

// ---------------------------------------------------------------- 해피 케이스

#[tokio::test]
async fn manual_scenario_end_to_end() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    manual_scenario(&db, p, Uuid::new_v4()).await;

    let rep = job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    assert_eq!(rep.orders_seen, 1);
    assert_eq!(rep.orders_written, 1);
    assert_eq!(rep.rows_written, 20);

    let rows = finals(&db, p).await;
    assert_manual(&rows, 100_000);
    // 어시스트: last_click에서 0원이 된 네이버
    let assist: Vec<(u16, u8)> = rows
        .iter()
        .filter(|r| r.model == "last_click" && r.lookback_days == 7)
        .map(|r| (r.touch_index, r.is_assist))
        .collect();
    assert_eq!(assist, vec![(1, 1), (2, 0)]);
    assert_eq!(
        rows.iter()
            .find(|r| r.channel == "meta")
            .map(|r| r.utm_campaign.as_str()),
        Some("retarget")
    );
    // 성공 후 watermark = 처리한 최대 version
    assert_eq!(
        settings::get_watermark(&db.pg, job::JOB_INCREMENTAL, p)
            .await
            .unwrap(),
        Some(1_000)
    );
    assert!(
        job::conservation_check(&db.ch, &db.pg, Some(p))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn run_all_projects_skips_inactive() {
    let db = TestDb::new().await;
    let a = project(&db.pg).await;
    let paused = project_with_status(&db.pg, "paused").await;
    let b = project(&db.pg).await;
    for p in [a, paused, b] {
        manual_scenario(&db, p, Uuid::new_v4()).await;
    }
    let rep = job::run(&db.ch, &db.pg, None, &Mode::Incremental, NOW)
        .await
        .unwrap();
    assert_eq!(rep.projects, 2);
    assert_eq!(rep.orders_written, 2);
    assert_manual(&finals(&db, a).await, 100_000);
    assert_manual(&finals(&db, b).await, 100_000);
    assert!(finals(&db, paused).await.is_empty());
    // --project로 지정하면 상태와 관계없이 실행한다 (운영자 수동 재연산)
    job::run(&db.ch, &db.pg, Some(paused), &Mode::Incremental, NOW)
        .await
        .unwrap();
    assert_manual(&finals(&db, paused).await, 100_000);
}

#[tokio::test]
async fn organic_and_referral_included_by_default_direct_excluded() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    let v = Uuid::new_v4();
    insert_touches(
        &db.ch,
        &[
            TouchFixture::referrer(p, v, ORDER_AT - 30 * MIN, "www.google.com"),
            TouchFixture::referrer(p, v, ORDER_AT - 20 * MIN, "news.example.org"),
            // direct·내부 이동은 터치포인트로 들어와도 기여 대상이 아니다
            TouchFixture::referrer(p, v, ORDER_AT - 10 * MIN, ""),
            TouchFixture::referrer(p, v, ORDER_AT - 5 * MIN, "www.shop.example.com"),
        ],
    )
    .await
    .unwrap();
    insert_orders(
        &db.ch,
        &[OrderFixture::confirmed(
            p,
            "O1",
            Some(v),
            10_000,
            ORDER_AT,
            1,
        )],
    )
    .await
    .unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    let rows = finals(&db, p).await;
    assert_eq!(
        pick(&rows, "O1", "linear", 7),
        vec![("organic_search".into(), 5_000), ("referral".into(), 5_000)]
    );

    // 광고 매체만 기여하도록 설정을 바꾸고 전체 재연산 → 미귀속
    sqlx::query(
        "INSERT INTO attribution_settings (project_id, include_organic, include_referral) VALUES ($1, FALSE, FALSE)",
    )
    .bind(p as i32)
    .execute(&db.pg)
    .await
    .unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Full { since_days: 35 }, NOW)
        .await
        .unwrap();
    let rows = finals(&db, p).await;
    assert_eq!(
        pick(&rows, "O1", "last_click", 7),
        vec![("(unattributed)".into(), 10_000)]
    );
}

// ---------------------------------------------------------------- 멱등성

#[tokio::test]
async fn rerun_is_idempotent() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    manual_scenario(&db, p, Uuid::new_v4()).await;
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    let first = finals(&db, p).await;
    let raw = raw_count(&db, p).await;

    // 증분 재실행 (겹침 구간 때문에 같은 주문을 다시 읽어도) → 쓰지 않는다
    let rep = job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW + MIN)
        .await
        .unwrap();
    assert_eq!(rep.orders_written, 0);
    // 전체 재실행 두 번 → 쓰지 않는다
    for i in 0..2 {
        let rep = job::run_project(
            &db.ch,
            &db.pg,
            p,
            &Mode::Full { since_days: 35 },
            NOW + (i + 2) * MIN,
        )
        .await
        .unwrap();
        assert_eq!(rep.orders_seen, 1);
        assert_eq!(rep.orders_written, 0);
        assert_eq!(rep.rows_written, 0);
    }
    assert_eq!(finals(&db, p).await, first);
    assert_eq!(raw_count(&db, p).await, raw);
}

// ---------------------------------------------------------------- 경계값·엣지

#[tokio::test]
async fn lookback_boundary_and_future_touch() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    let v = Uuid::new_v4();
    insert_touches(
        &db.ch,
        &[
            // 30일 경계 정확히: 30일에 포함
            TouchFixture::utm(p, v, ORDER_AT - 30 * DAY_MS, "kakao", "", ""),
            // 30일 경계 1ms 전: 어디에도 없다
            TouchFixture::utm(p, v, ORDER_AT - 30 * DAY_MS - 1, "tiktok", "", ""),
            // 7일 경계 1ms 전: 30일에만
            TouchFixture::utm(p, v, ORDER_AT - 7 * DAY_MS - 1, "criteo", "", ""),
            // 7일 경계 정확히: 7일에 포함
            TouchFixture::utm(p, v, ORDER_AT - 7 * DAY_MS, "naver", "cpc", ""),
            // 결제 시각과 같은 ms: 포함
            TouchFixture::utm(p, v, ORDER_AT, "facebook", "", ""),
            // 결제 이후: 제외
            TouchFixture::utm(p, v, ORDER_AT + 1, "google", "cpc", ""),
        ],
    )
    .await
    .unwrap();
    insert_orders(
        &db.ch,
        &[OrderFixture::confirmed(
            p,
            "O1",
            Some(v),
            1_000,
            ORDER_AT,
            1,
        )],
    )
    .await
    .unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    let rows = finals(&db, p).await;
    let chans = |lb| -> Vec<String> {
        pick(&rows, "O1", "raw_sum", lb)
            .into_iter()
            .map(|x| x.0)
            .collect()
    };
    assert_eq!(chans(7), vec!["naver_sa", "meta"]);
    assert_eq!(chans(30), vec!["kakao", "criteo", "naver_sa", "meta"]);
    assert_eq!(
        pick(&rows, "O1", "last_click", 30).last().unwrap(),
        &("meta".to_string(), 1_000)
    );
}

#[tokio::test]
async fn zero_amount_order_writes_nothing() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    let v = Uuid::new_v4();
    insert_touches(
        &db.ch,
        &[TouchFixture::utm(p, v, ORDER_AT - MIN, "naver", "cpc", "")],
    )
    .await
    .unwrap();
    insert_orders(
        &db.ch,
        &[OrderFixture::confirmed(p, "FREE", Some(v), 0, ORDER_AT, 1)],
    )
    .await
    .unwrap();
    let rep = job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    assert_eq!(rep.orders_seen, 1);
    assert_eq!(rep.rows_written, 0);
    assert_eq!(raw_count(&db, p).await, 0);
    assert!(
        job::conservation_check(&db.ch, &db.pg, Some(p))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn order_without_visitor_uses_member_hash() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    let (mobile, pc) = (Uuid::new_v4(), Uuid::new_v4());
    insert_links(
        &db.ch,
        &[
            IdentityFixture::new(p, "member-1", mobile),
            IdentityFixture::new(p, "member-1", pc),
        ],
    )
    .await
    .unwrap();
    insert_touches(
        &db.ch,
        &[
            TouchFixture::utm(p, mobile, ORDER_AT - 20 * MIN, "naver", "cpc", ""),
            TouchFixture::utm(p, pc, ORDER_AT - 10 * MIN, "kakao", "", ""),
        ],
    )
    .await
    .unwrap();
    let mut o = OrderFixture::confirmed(p, "S2S-1", None, 9_000, ORDER_AT, 1);
    o.member_hash = "member-1".into();
    insert_orders(&db.ch, &[o]).await.unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    let rows = finals(&db, p).await;
    assert_eq!(
        pick(&rows, "S2S-1", "linear", 7),
        vec![("naver_sa".into(), 4_500), ("kakao".into(), 4_500)]
    );
}

#[tokio::test]
async fn no_touches_is_unattributed() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    insert_orders(
        &db.ch,
        &[
            OrderFixture::confirmed(p, "O1", Some(Uuid::new_v4()), 7_777, ORDER_AT, 1),
            // 식별자가 전혀 없는 주문
            OrderFixture::confirmed(p, "O2", None, 3_000, ORDER_AT, 2),
        ],
    )
    .await
    .unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    let rows = finals(&db, p).await;
    assert_eq!(rows.len(), 20);
    for m in [
        "last_click",
        "first_click",
        "linear",
        "time_decay",
        "raw_sum",
    ] {
        assert_eq!(
            pick(&rows, "O1", m, 30),
            vec![("(unattributed)".into(), 7_777)]
        );
        assert_eq!(
            pick(&rows, "O2", m, 7),
            vec![("(unattributed)".into(), 3_000)]
        );
    }
    assert!(rows.iter().all(|r| r.touch_index == 0));
}

#[tokio::test]
async fn thousand_touches() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    let v = Uuid::new_v4();
    let chans = [("naver", "cpc"), ("facebook", ""), ("kakao", "")];
    let touches: Vec<TouchFixture> = (0..1_000)
        .map(|i| {
            let (s, m) = chans[i % 3];
            TouchFixture::utm(p, v, ORDER_AT - (i as i64 + 1) * MIN, s, m, "")
        })
        .collect();
    insert_touches(&db.ch, &touches).await.unwrap();
    insert_orders(
        &db.ch,
        &[OrderFixture::confirmed(
            p,
            "BIG",
            Some(v),
            1_000_003,
            ORDER_AT,
            1,
        )],
    )
    .await
    .unwrap();
    let rep = job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    assert_eq!(rep.rows_written, 2 * 5 * 1_000);
    let rows = finals(&db, p).await;
    for lb in [7, 30] {
        for m in ["last_click", "first_click", "linear", "time_decay"] {
            assert_eq!(total(&rows, "BIG", m, lb), 1_000_003, "{m}");
        }
        assert_eq!(total(&rows, "BIG", "raw_sum", lb), 3 * 1_000_003);
    }
    assert!(
        job::conservation_check(&db.ch, &db.pg, Some(p))
            .await
            .unwrap()
            .is_empty()
    );
}

// ---------------------------------------------------------------- 희귀 케이스

#[tokio::test]
async fn late_touchpoint_is_picked_up_by_full_run() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    let v = Uuid::new_v4();
    insert_orders(
        &db.ch,
        &[OrderFixture::confirmed(
            p,
            "O1",
            Some(v),
            5_000,
            ORDER_AT,
            1,
        )],
    )
    .await
    .unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    assert_eq!(
        pick(&finals(&db, p).await, "O1", "last_click", 7),
        vec![("(unattributed)".into(), 5_000)]
    );

    // 결제 전 시각의 터치포인트가 늦게 적재됨
    insert_touches(
        &db.ch,
        &[TouchFixture::utm(
            p,
            v,
            ORDER_AT - 3 * MIN,
            "naver",
            "cpc",
            "",
        )],
    )
    .await
    .unwrap();
    // 주문 version이 그대로라 증분에서는 바뀌지 않는다
    let rep = job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW + MIN)
        .await
        .unwrap();
    assert_eq!(rep.orders_written, 0);

    let rep = job::run_project(
        &db.ch,
        &db.pg,
        p,
        &Mode::Full { since_days: 35 },
        NOW + 2 * MIN,
    )
    .await
    .unwrap();
    assert_eq!(rep.orders_written, 1);
    let rows = finals(&db, p).await;
    assert_eq!(
        pick(&rows, "O1", "last_click", 7),
        vec![("naver_sa".into(), 5_000)]
    );
    // 미귀속(touch_index 0) 행은 삭제 표시로 사라졌다
    assert!(rows.iter().all(|r| r.touch_index != 0));
    let deleted: u64 = db
        .ch
        .query("SELECT count() FROM attributed_revenue FINAL WHERE touch_index = 0")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(deleted, 0);
}

#[tokio::test]
async fn cancel_after_attribution_marks_rows_deleted() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    let v = Uuid::new_v4();
    manual_scenario(&db, p, v).await;
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    assert_eq!(finals(&db, p).await.len(), 20);

    let mut cancelled = OrderFixture::confirmed(p, "ORD-9901", Some(v), 100_000, ORDER_AT, 2_000);
    cancelled.status = "CANCELLED".into();
    cancelled.effective_amount = 0;
    insert_orders(&db.ch, &[cancelled]).await.unwrap();
    let rep = job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW + MIN)
        .await
        .unwrap();
    assert_eq!(rep.rows_written, 20);
    assert!(finals(&db, p).await.is_empty());
    let marked: u64 = db
        .ch
        .query(
            "SELECT count() FROM attributed_revenue \
             WHERE project_id = ? AND is_deleted = 1 AND order_id = 'ORD-9901'",
        )
        .bind(p)
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(marked, 20);
    // 다시 돌려도 더 쓰지 않는다
    let rep = job::run_project(
        &db.ch,
        &db.pg,
        p,
        &Mode::Full { since_days: 35 },
        NOW + 2 * MIN,
    )
    .await
    .unwrap();
    assert_eq!(rep.rows_written, 0);
    assert!(
        job::conservation_check(&db.ch, &db.pg, Some(p))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn partial_refund_lowers_amounts() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    let v = Uuid::new_v4();
    manual_scenario(&db, p, v).await;
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();

    let mut refunded = OrderFixture::confirmed(p, "ORD-9901", Some(v), 100_000, ORDER_AT, 2_000);
    refunded.status = "PARTIALLY_REFUNDED".into();
    refunded.refunded_amount = 30_000;
    refunded.effective_amount = 70_000;
    insert_orders(&db.ch, &[refunded]).await.unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW + MIN)
        .await
        .unwrap();
    assert_manual(&finals(&db, p).await, 70_000);
    assert_eq!(
        settings::get_watermark(&db.pg, job::JOB_INCREMENTAL, p)
            .await
            .unwrap(),
        Some(2_000)
    );
    assert!(
        job::conservation_check(&db.ch, &db.pg, Some(p))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn channel_rule_change_relabels_retroactively_on_full_run() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    manual_scenario(&db, p, Uuid::new_v4()).await;
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO channel_rules (project_id, priority, channel, match) VALUES ($1, 1, 'naver_brand', $2)",
    )
    .bind(p as i32)
    .bind(serde_json::json!({ "utm_source": ["naver"], "utm_campaign_prefix": ["brand"] }))
    .execute(&db.pg)
    .await
    .unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Full { since_days: 35 }, NOW + MIN)
        .await
        .unwrap();
    let rows = finals(&db, p).await;
    assert_eq!(
        pick(&rows, "ORD-9901", "first_click", 7),
        vec![("naver_brand".into(), 100_000), ("meta".into(), 0)]
    );
    assert!(rows.iter().all(|r| r.channel != "naver_sa"));
}

#[tokio::test]
async fn identity_link_added_later_merges_other_device() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    let (mobile, pc) = (Uuid::new_v4(), Uuid::new_v4());
    // 모바일에서 광고 클릭, PC에서 로그인 후 결제
    insert_touches(
        &db.ch,
        &[TouchFixture::utm(
            p,
            mobile,
            ORDER_AT - DAY_MS,
            "facebook",
            "",
            "",
        )],
    )
    .await
    .unwrap();
    let mut o = OrderFixture::confirmed(p, "X1", Some(pc), 50_000, ORDER_AT, 1);
    o.member_hash = "m".into();
    insert_orders(&db.ch, &[o]).await.unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    assert_eq!(
        pick(&finals(&db, p).await, "X1", "last_click", 7),
        vec![("(unattributed)".into(), 50_000)]
    );

    // 모바일에서도 로그인 → identity 연결
    insert_links(
        &db.ch,
        &[
            IdentityFixture::new(p, "m", mobile),
            IdentityFixture::new(p, "m", pc),
        ],
    )
    .await
    .unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Full { since_days: 35 }, NOW + MIN)
        .await
        .unwrap();
    assert_eq!(
        pick(&finals(&db, p).await, "X1", "last_click", 7),
        vec![("meta".into(), 50_000)]
    );
}

#[tokio::test]
async fn two_touches_same_ms_are_deterministic() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    let (v1, v2) = (Uuid::new_v4(), Uuid::new_v4());
    insert_links(
        &db.ch,
        &[
            IdentityFixture::new(p, "m", v1),
            IdentityFixture::new(p, "m", v2),
        ],
    )
    .await
    .unwrap();
    let ts = ORDER_AT - 10 * MIN;
    insert_touches(
        &db.ch,
        &[
            TouchFixture::utm(p, v1, ts, "naver", "cpc", ""),
            TouchFixture::utm(p, v2, ts, "facebook", "", ""),
        ],
    )
    .await
    .unwrap();
    let mut o = OrderFixture::confirmed(p, "T", Some(v1), 100_001, ORDER_AT, 1);
    o.member_hash = "m".into();
    insert_orders(&db.ch, &[o]).await.unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    let first = finals(&db, p).await;
    // 같은 ms면 채널 이름 순: meta → naver_sa, 나머지 1원은 마지막 접점
    assert_eq!(
        pick(&first, "T", "linear", 7),
        vec![("meta".into(), 50_000), ("naver_sa".into(), 50_001)]
    );
    assert_eq!(
        pick(&first, "T", "last_click", 7),
        vec![("meta".into(), 0), ("naver_sa".into(), 100_001)]
    );
    let rep = job::run_project(&db.ch, &db.pg, p, &Mode::Full { since_days: 35 }, NOW + MIN)
        .await
        .unwrap();
    assert_eq!(rep.orders_written, 0);
    assert_eq!(finals(&db, p).await, first);
}

#[tokio::test]
async fn stale_version_below_watermark_is_caught_by_full_run() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    insert_orders(
        &db.ch,
        &[OrderFixture::confirmed(
            p, "NEW", None, 1_000, ORDER_AT, 10_000_000,
        )],
    )
    .await
    .unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    // watermark보다 한참 낮은 version으로 늦게 커밋된 주문
    insert_orders(
        &db.ch,
        &[OrderFixture::confirmed(p, "LATE", None, 2_000, ORDER_AT, 1)],
    )
    .await
    .unwrap();
    let rep = job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW + MIN)
        .await
        .unwrap();
    assert_eq!(rep.orders_written, 0);
    assert!(pick(&finals(&db, p).await, "LATE", "linear", 7).is_empty());
    // 전체 재연산은 watermark를 건드리지 않고 빠진 주문을 채운다
    job::run_project(
        &db.ch,
        &db.pg,
        p,
        &Mode::Full { since_days: 35 },
        NOW + 2 * MIN,
    )
    .await
    .unwrap();
    assert_eq!(total(&finals(&db, p).await, "LATE", "linear", 7), 2_000);
    assert_eq!(
        settings::get_watermark(&db.pg, job::JOB_INCREMENTAL, p)
            .await
            .unwrap(),
        Some(10_000_000)
    );
}

#[tokio::test]
async fn full_run_respects_since_days() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    insert_orders(
        &db.ch,
        &[
            OrderFixture::confirmed(p, "RECENT", None, 1, NOW - 2 * DAY_MS, 1),
            OrderFixture::confirmed(p, "OLD", None, 1, NOW - 40 * DAY_MS, 2),
        ],
    )
    .await
    .unwrap();
    let rep = job::run_project(&db.ch, &db.pg, p, &Mode::Full { since_days: 35 }, NOW)
        .await
        .unwrap();
    assert_eq!(rep.orders_seen, 1);
    let rows = finals(&db, p).await;
    assert!(rows.iter().all(|r| r.order_id == "RECENT"));
    // 전체 재연산은 watermark를 만들지 않는다
    assert_eq!(
        settings::get_watermark(&db.pg, job::JOB_INCREMENTAL, p)
            .await
            .unwrap(),
        None
    );
}

// ---------------------------------------------------------------- 테넌트 격리

#[tokio::test]
async fn touches_never_cross_projects() {
    let db = TestDb::new().await;
    let a = project(&db.pg).await;
    let b = project(&db.pg).await;
    let shared_visitor = Uuid::new_v4();
    // A에만 터치포인트·identity, B에 같은 visitor_id·같은 주문 ID·같은 회원 해시의 주문
    insert_touches(
        &db.ch,
        &[TouchFixture::utm(
            a,
            shared_visitor,
            ORDER_AT - MIN,
            "naver",
            "cpc",
            "",
        )],
    )
    .await
    .unwrap();
    insert_links(&db.ch, &[IdentityFixture::new(a, "m", shared_visitor)])
        .await
        .unwrap();
    let mut ob = OrderFixture::confirmed(b, "SAME-ID", Some(shared_visitor), 10_000, ORDER_AT, 1);
    ob.member_hash = "m".into();
    insert_orders(&db.ch, &[ob]).await.unwrap();

    job::run(&db.ch, &db.pg, None, &Mode::Incremental, NOW)
        .await
        .unwrap();
    assert!(finals(&db, a).await.is_empty());
    let rows_b = finals(&db, b).await;
    assert_eq!(
        pick(&rows_b, "SAME-ID", "last_click", 30),
        vec![("(unattributed)".into(), 10_000)]
    );
    // A의 watermark는 B 주문으로 움직이지 않는다
    assert_eq!(
        settings::get_watermark(&db.pg, job::JOB_INCREMENTAL, a)
            .await
            .unwrap(),
        None
    );
}

// ---------------------------------------------------------------- 보존 검증

#[tokio::test]
async fn conservation_check_detects_corruption_and_recompute_fixes_it() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    manual_scenario(&db, p, Uuid::new_v4()).await;
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    assert!(
        job::conservation_check(&db.ch, &db.pg, None)
            .await
            .unwrap()
            .is_empty()
    );

    // 의도적으로 손상: linear 7일 첫 접점 금액을 더 새 computed_at으로 덮어쓴다
    db.ch
        .query(
            "INSERT INTO attributed_revenue \
             SELECT project_id, model, lookback_days, order_id, touch_index, channel, utm_campaign, \
                    touch_ts, ordered_at, amount + 1, is_assist, computed_at + 1000, is_deleted \
             FROM attributed_revenue FINAL \
             WHERE model = 'linear' AND lookback_days = 7 AND touch_index = 1",
        )
        .execute()
        .await
        .unwrap();
    let broken = job::conservation_check(&db.ch, &db.pg, None).await.unwrap();
    assert_eq!(broken.len(), 1);
    let (bp, b) = &broken[0];
    assert_eq!(*bp, p);
    assert_eq!(
        (b.model.as_str(), b.lookback_days, b.order_id.as_str()),
        ("linear", 7, "ORD-9901")
    );
    assert_eq!((b.attributed, b.effective_amount), (100_001, 100_000));

    // 깨진 주문만 재연산 → 복구
    let rep = job::run_project(
        &db.ch,
        &db.pg,
        p,
        &Mode::Orders(vec!["ORD-9901".into()]),
        NOW + MIN,
    )
    .await
    .unwrap();
    assert_eq!(rep.orders_written, 1);
    assert!(
        job::conservation_check(&db.ch, &db.pg, Some(p))
            .await
            .unwrap()
            .is_empty()
    );
    assert_manual(&finals(&db, p).await, 100_000);
}

// ---------------------------------------------------------------- 장애·복구

#[tokio::test]
async fn failed_write_keeps_watermark_and_retry_succeeds() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    manual_scenario(&db, p, Uuid::new_v4()).await;
    db.ch
        .query("RENAME TABLE attributed_revenue TO attributed_revenue_away")
        .execute()
        .await
        .unwrap();
    assert!(
        job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
            .await
            .is_err()
    );
    assert_eq!(
        settings::get_watermark(&db.pg, job::JOB_INCREMENTAL, p)
            .await
            .unwrap(),
        None
    );
    db.ch
        .query("RENAME TABLE attributed_revenue_away TO attributed_revenue")
        .execute()
        .await
        .unwrap();
    job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW + MIN)
        .await
        .unwrap();
    assert_manual(&finals(&db, p).await, 100_000);
}

#[tokio::test]
async fn many_orders_span_multiple_batches() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    let n = job::BATCH_ORDERS + 37;
    let mut touches = Vec::new();
    let mut orders = Vec::new();
    for i in 0..n {
        let v = Uuid::new_v4();
        touches.push(TouchFixture::utm(p, v, ORDER_AT - MIN, "naver", "cpc", ""));
        orders.push(OrderFixture::confirmed(
            p,
            &format!("O{i:04}"),
            Some(v),
            1_000 + i as i64,
            ORDER_AT,
            100 + i as u64,
        ));
    }
    insert_touches(&db.ch, &touches).await.unwrap();
    insert_orders(&db.ch, &orders).await.unwrap();
    let rep = job::run_project(&db.ch, &db.pg, p, &Mode::Incremental, NOW)
        .await
        .unwrap();
    assert_eq!(rep.orders_written, n);
    assert_eq!(
        settings::get_watermark(&db.pg, job::JOB_INCREMENTAL, p)
            .await
            .unwrap(),
        Some(100 + n as i64 - 1)
    );
    let rows = finals(&db, p).await;
    assert_eq!(rows.len(), n * 10);
    assert!(rows.iter().all(|r| r.channel == "naver_sa"));
}

// ---------------------------------------------------------------- 워커·CLI

#[tokio::test]
async fn worker_runs_incremental_and_stops_on_shutdown() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    manual_scenario(&db, p, Uuid::new_v4()).await;
    let cfg = worker::WorkerConfig {
        incremental_every: Duration::from_millis(200),
        ..Default::default()
    };
    tokio::time::timeout(
        Duration::from_secs(30),
        worker::run(
            &db.ch,
            &db.pg,
            &cfg,
            tokio::time::sleep(Duration::from_millis(1_500)),
        ),
    )
    .await
    .expect("shutdown 신호 후 종료해야 한다")
    .unwrap();
    assert_manual(&finals(&db, p).await, 100_000);
}

fn cli(db: &TestDb, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_analytics-jobs"))
        .args(args)
        .env("DATABASE_URL", &db.pg_url)
        .env("CLICKHOUSE_URL", &db.ch_config.url)
        .env("CLICKHOUSE_USER", &db.ch_config.user)
        .env("CLICKHOUSE_PASSWORD", &db.ch_config.password)
        .env("CLICKHOUSE_DATABASE", &db.ch_config.database)
        .output()
        .unwrap()
}

#[tokio::test]
async fn cli_attribution_and_conservation_check() {
    let db = TestDb::new().await;
    let p = project(&db.pg).await;
    // 실제 시각 기준으로 돌기 때문에 최근 주문을 쓴다
    let now = job::now_ms();
    let v = Uuid::new_v4();
    insert_touches(
        &db.ch,
        &[TouchFixture::utm(p, v, now - 10 * MIN, "naver", "cpc", "")],
    )
    .await
    .unwrap();
    insert_orders(
        &db.ch,
        &[OrderFixture::confirmed(
            p,
            "C1",
            Some(v),
            5_000,
            now - MIN,
            1,
        )],
    )
    .await
    .unwrap();

    let out = cli(&db, &["attribution", "--project", &p.to_string()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = cli(&db, &["attribution", "--full", "--since-days", "35"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(total(&finals(&db, p).await, "C1", "last_click", 7), 5_000);

    let ok = cli(&db, &["conservation-check", "--project", &p.to_string()]);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );

    db.ch
        .query(
            "INSERT INTO attributed_revenue \
             SELECT project_id, model, lookback_days, order_id, touch_index, channel, utm_campaign, \
                    touch_ts, ordered_at, amount - 1, is_assist, computed_at + 1000, is_deleted \
             FROM attributed_revenue FINAL WHERE model = 'first_click' AND lookback_days = 30",
        )
        .execute()
        .await
        .unwrap();
    let bad = cli(&db, &["conservation-check"]);
    assert!(!bad.status.success());
    let stdout = String::from_utf8_lossy(&bad.stdout);
    assert!(
        stdout.contains("C1") && stdout.contains("first_click"),
        "{stdout}"
    );
}

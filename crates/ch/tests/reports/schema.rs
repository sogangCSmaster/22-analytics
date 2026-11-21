//! 롤업 테이블(0004)·마이그레이션·클라이언트 설정.
//!
//! 롤업(`daily_channel`·`daily_pages`·`daily_events`)은 원본 `events`를 정확 집계(`uniqExact`)한
//! 값과 같아야 한다. 픽스처는 작아서 `uniqCombined(14)`도 정확하다.

use crate::common::*;
use analytics_ch::ChConfig;
use analytics_ch::migrate::{self, MIGRATIONS};
use analytics_testkit::TestDb;
use time::Duration;

/// 여러 프로젝트·일자(타임존 경계)·채널·호스트·경로·이벤트 이름, 이벤트로 시작하는 세션.
fn rollup_fixture() -> Vec<Ev> {
    let mut evs = vec![
        ev(P, "page_view", 1, 11, "2026-11-20 10:00:00").start("naver_sa"),
        ev(P, "page_view", 1, 11, "2026-11-20 10:01:00")
            .ch("naver_sa")
            .path("/goods/a"),
        ev(P, "add_to_cart", 1, 11, "2026-11-20 10:02:00")
            .ch("naver_sa")
            .path("/goods/a"),
        ev(P, "page_view", 2, 21, "2026-11-20 11:00:00").start("meta"),
        ev(P, "page_view", 2, 21, "2026-11-20 11:05:00")
            .ch("meta")
            .path("/cart"),
        // 페이지뷰가 아닌 이벤트로 시작한 세션
        ev(P, "signup", 3, 31, "2026-11-20 23:59:59")
            .start("kakao")
            .path("/join"),
        ev(P, "page_view", 3, 31, "2026-11-21 00:00:00")
            .ch("kakao")
            .path("/join"),
        // KST 11-01 00:30 = UTC 10-31 15:30
        ev(P, "page_view", 4, 41, "2026-11-01 00:30:00").start("direct"),
        ev(P, "page_view", 5, 51, "2026-10-31 12:00:00").start("direct"),
        ev(Q, "page_view", 1, 11, "2026-11-20 10:00:00")
            .start("kakao")
            .path("/q-only"),
    ];
    evs.push(
        ev(P, "page_view", 6, 61, "2026-11-20 12:00:00")
            .start("meta")
            .host("m.shop.test"),
    );
    for v in 0..30u128 {
        evs.push(ev(P, "view_item", 100 + v, 200 + v, "2026-11-21 13:00:00").ch("meta"));
    }
    evs
}

/// 정렬된 튜플 배열을 문자열로 받는다 (행 순서와 무관하게 비교).
async fn tuples(ch: &analytics_ch::Client, sql: &str) -> String {
    ch.query(&format!(
        "SELECT toString(arraySort(groupArray(r))) FROM ({sql})"
    ))
    .fetch_one::<String>()
    .await
    .unwrap_or_else(|e| panic!("{e}\n{sql}"))
}

async fn assert_rollups_match_raw(ch: &analytics_ch::Client) {
    let raw_channel = tuples(
        ch,
        "SELECT tuple(project_id, toDate(ts, 'Asia/Seoul'), toString(channel),
                      countIf(event_name = 'page_view'), count(), countIf(is_session_start = 1),
                      uniqExact(visitor_id), uniqExact(session_id)) AS r
         FROM events GROUP BY project_id, toDate(ts, 'Asia/Seoul'), channel",
    )
    .await;
    let rollup_channel = tuples(
        ch,
        "SELECT tuple(project_id, day, toString(channel),
                      sum(pageviews), sum(events), sum(entrances),
                      uniqCombinedMerge(14)(visitors), uniqCombinedMerge(14)(sessions)) AS r
         FROM daily_channel GROUP BY project_id, day, channel",
    )
    .await;
    assert_eq!(rollup_channel, raw_channel, "daily_channel");

    let raw_pages = tuples(
        ch,
        "SELECT tuple(project_id, toDate(ts, 'Asia/Seoul'), toString(host), path,
                      countIf(event_name = 'page_view'), countIf(is_session_start = 1),
                      uniqExactIf(visitor_id, event_name = 'page_view')) AS r
         FROM events WHERE event_name = 'page_view' OR is_session_start = 1
         GROUP BY project_id, toDate(ts, 'Asia/Seoul'), host, path",
    )
    .await;
    let rollup_pages = tuples(
        ch,
        "SELECT tuple(project_id, day, toString(host), path,
                      sum(pageviews), sum(entrances), uniqCombinedMerge(14)(visitors)) AS r
         FROM daily_pages GROUP BY project_id, day, host, path",
    )
    .await;
    assert_eq!(rollup_pages, raw_pages, "daily_pages");

    let raw_events = tuples(
        ch,
        "SELECT tuple(project_id, toDate(ts, 'Asia/Seoul'), toString(event_name),
                      count(), uniqExact(visitor_id)) AS r
         FROM events GROUP BY project_id, toDate(ts, 'Asia/Seoul'), event_name",
    )
    .await;
    let rollup_events = tuples(
        ch,
        "SELECT tuple(project_id, day, toString(event_name),
                      sum(events), uniqCombinedMerge(14)(visitors)) AS r
         FROM daily_events GROUP BY project_id, day, event_name",
    )
    .await;
    assert_eq!(rollup_events, raw_events, "daily_events");
    assert_ne!(raw_events, "[]", "픽스처가 비어 있으면 안 된다");
}

#[tokio::test]
async fn rollups_agree_with_raw_events() {
    let db = TestDb::new().await;
    let evs = rollup_fixture();
    let half = evs.len() / 2;
    let mut evs = evs.into_iter();
    // 여러 INSERT(병합 전 파트 여러 개)로 나눠 넣는다
    insert(&db.ch, evs.by_ref().take(half).collect()).await;
    insert(&db.ch, evs.collect()).await;
    assert_rollups_match_raw(&db.ch).await;
    // 병합 후에도 같다
    for t in ["daily_channel", "daily_pages", "daily_events"] {
        db.ch
            .query(&format!("OPTIMIZE TABLE {t} FINAL"))
            .execute()
            .await
            .unwrap();
    }
    assert_rollups_match_raw(&db.ch).await;
}

#[tokio::test]
async fn old_daily_stats_is_dropped() {
    let db = TestDb::new().await;
    let names: Vec<String> = db
        .ch
        .query("SELECT name FROM system.tables WHERE database = currentDatabase() ORDER BY name")
        .fetch_all()
        .await
        .unwrap();
    for t in [
        "daily_channel",
        "mv_daily_channel",
        "daily_pages",
        "mv_daily_pages",
        "daily_events",
        "mv_daily_events",
    ] {
        assert!(names.iter().any(|n| n == t), "{t} 없음: {names:?}");
    }
    assert!(!names.iter().any(|n| n == "daily_stats"));
    assert!(!names.iter().any(|n| n == "mv_daily_stats"));
}

/// 0001만 적용된 DB (0004 이전 운영 상태 재현). drop되면 지운다.
struct LegacyDb {
    cfg: ChConfig,
}

impl LegacyDb {
    async fn new() -> Self {
        let mut cfg = ChConfig::from_env();
        cfg.database = format!("test_{}", uuid::Uuid::new_v4().simple());
        cfg.server_client()
            .query(&format!("CREATE DATABASE `{}`", cfg.database))
            .execute()
            .await
            .unwrap();
        let ch = cfg.client();
        ch.query(
            "CREATE TABLE schema_migrations \
             (version String, applied_at DateTime DEFAULT now()) \
             ENGINE = ReplacingMergeTree ORDER BY version",
        )
        .execute()
        .await
        .unwrap();
        let (version, sql) = MIGRATIONS[0];
        assert_eq!(version, "0001_init");
        for stmt in migrate::statements(sql) {
            ch.query(&stmt).execute().await.unwrap();
        }
        ch.query("INSERT INTO schema_migrations (version) VALUES (?)")
            .bind(version)
            .execute()
            .await
            .unwrap();
        Self { cfg }
    }

    fn client(&self) -> analytics_ch::Client {
        self.cfg
            .client()
            .with_setting("insert_deduplicate", "0")
            .with_setting("wait_end_of_query", "1")
    }
}

impl Drop for LegacyDb {
    fn drop(&mut self) {
        let cfg = self.cfg.clone();
        let _ = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                let _ = cfg
                    .server_client()
                    .query(&format!("DROP DATABASE IF EXISTS `{}`", cfg.database))
                    .execute()
                    .await;
            });
        })
        .join();
    }
}

#[tokio::test]
async fn migration_0004_backfills_existing_events_and_is_idempotent() {
    let legacy = LegacyDb::new().await;
    let ch = legacy.client();
    // 0004 이전에 쌓인 이벤트
    insert(&ch, rollup_fixture()).await;

    let applied = migrate::run(&legacy.cfg).await.unwrap();
    assert!(applied.contains(&"0004_rollups"), "{applied:?}");
    assert_rollups_match_raw(&ch).await;

    // 이후 적재는 MV가 채운다
    let base = kst("2026-11-22 10:00:00");
    insert(
        &ch,
        (0..5u128)
            .map(|i| {
                ev_at(
                    P,
                    "page_view",
                    500 + i,
                    600 + i,
                    base + Duration::minutes(i as i64),
                )
                .start("google_ads")
                .path("/new")
            })
            .collect(),
    )
    .await;
    assert_rollups_match_raw(&ch).await;

    // 다시 실행해도 아무것도 적용되지 않고 롤업이 두 번 쌓이지 않는다
    let again = migrate::run(&legacy.cfg).await.unwrap();
    assert!(again.is_empty(), "{again:?}");
    assert_rollups_match_raw(&ch).await;
}

#[tokio::test]
async fn client_has_query_time_limit() {
    let db = TestDb::new().await;
    let v: String = db
        .ch_config
        .client()
        .query("SELECT toString(getSetting('max_execution_time'))")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(v, "10");
}

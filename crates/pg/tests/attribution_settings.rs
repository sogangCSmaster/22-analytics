//! 기여 설정·watermark 통합 테스트.

use analytics_core::channel::RuleMatch;
use analytics_pg::attribution_settings::{self as s, AttributionSettings};
use analytics_testkit::TestDb;
use sqlx::PgPool;

async fn project(pool: &PgPool, status: &str) -> u32 {
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

#[tokio::test(flavor = "multi_thread")]
async fn defaults_when_row_missing() {
    let db = TestDb::new().await;
    let p = project(&db.pg, "active").await;
    let got = s::load(&db.pg, p).await.unwrap();
    assert_eq!(got, AttributionSettings::default());
    assert_eq!(got.lookback_days, vec![7, 30]);
    assert_eq!(got.half_life_hours, 168);
    // D4: 자연검색·추천은 기본 포함
    assert!(got.include_organic && got.include_referral);
    assert_eq!(got.max_lookback_days(), 30);
    // 프로젝트 자체가 없어도 기본값
    assert_eq!(
        s::load(&db.pg, 999_999).await.unwrap(),
        AttributionSettings::default()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn loads_stored_row() {
    let db = TestDb::new().await;
    let p = project(&db.pg, "active").await;
    sqlx::query(
        "INSERT INTO attribution_settings \
         (project_id, lookback_days, half_life_hours, include_organic, include_referral) \
         VALUES ($1, ARRAY[14, 1]::SMALLINT[], 24, FALSE, FALSE)",
    )
    .bind(p as i32)
    .execute(&db.pg)
    .await
    .unwrap();
    let got = s::load(&db.pg, p).await.unwrap();
    assert_eq!(
        got,
        AttributionSettings {
            lookback_days: vec![1, 14],
            half_life_hours: 24,
            include_organic: false,
            include_referral: false,
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sanitizes_invalid_values() {
    let db = TestDb::new().await;
    let p = project(&db.pg, "active").await;
    // 0, 음수, UInt8 범위 밖, 중복은 버린다. 반감기 0은 기본값.
    sqlx::query(
        "INSERT INTO attribution_settings (project_id, lookback_days, half_life_hours) \
         VALUES ($1, ARRAY[0, -3, 300, 30, 30, 7]::SMALLINT[], 0)",
    )
    .bind(p as i32)
    .execute(&db.pg)
    .await
    .unwrap();
    let got = s::load(&db.pg, p).await.unwrap();
    assert_eq!(got.lookback_days, vec![7, 30]);
    assert_eq!(got.half_life_hours, 168);

    // 유효한 값이 하나도 없으면 기본 lookback
    sqlx::query("UPDATE attribution_settings SET lookback_days = ARRAY[]::SMALLINT[]")
        .execute(&db.pg)
        .await
        .unwrap();
    assert_eq!(s::load(&db.pg, p).await.unwrap().lookback_days, vec![7, 30]);
}

#[tokio::test(flavor = "multi_thread")]
async fn watermark_roundtrip_and_isolation() {
    let db = TestDb::new().await;
    assert_eq!(
        s::get_watermark(&db.pg, "attribution", 1).await.unwrap(),
        None
    );
    s::set_watermark(&db.pg, "attribution", 1, 1_700_000_000_000)
        .await
        .unwrap();
    // 같은 값 두 번(멱등), 덮어쓰기
    s::set_watermark(&db.pg, "attribution", 1, 1_700_000_000_000)
        .await
        .unwrap();
    assert_eq!(
        s::get_watermark(&db.pg, "attribution", 1).await.unwrap(),
        Some(1_700_000_000_000)
    );
    s::set_watermark(&db.pg, "attribution", 1, 5).await.unwrap();
    assert_eq!(
        s::get_watermark(&db.pg, "attribution", 1).await.unwrap(),
        Some(5)
    );
    // 다른 프로젝트·다른 잡과 섞이지 않는다
    assert_eq!(
        s::get_watermark(&db.pg, "attribution", 2).await.unwrap(),
        None
    );
    assert_eq!(s::get_watermark(&db.pg, "other", 1).await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn channel_rules_project_first_then_global() {
    let db = TestDb::new().await;
    let p = project(&db.pg, "active").await;
    let other = project(&db.pg, "active").await;
    let m = serde_json::json!({ "utm_source": ["naver"] });
    for (pid, prio, ch) in [
        (None, 1, "global_first"),
        (Some(p as i32), 50, "proj_b"),
        (Some(p as i32), 10, "proj_a"),
        (Some(other as i32), 1, "other_project"),
    ] {
        sqlx::query(
            "INSERT INTO channel_rules (project_id, priority, channel, match) VALUES ($1, $2, $3, $4)",
        )
        .bind(pid)
        .bind(prio)
        .bind(ch)
        .bind(&m)
        .execute(&db.pg)
        .await
        .unwrap();
    }
    // 잘못된 match JSON은 건너뛴다
    sqlx::query(
        "INSERT INTO channel_rules (project_id, priority, channel, match) VALUES ($1, 0, 'bad', '\"x\"')",
    )
    .bind(p as i32)
    .execute(&db.pg)
    .await
    .unwrap();

    let rules = s::channel_rules(&db.pg, p).await.unwrap();
    let names: Vec<&str> = rules.iter().map(|r| r.channel.as_str()).collect();
    assert_eq!(names, vec!["proj_a", "proj_b", "global_first"]);
    assert_eq!(
        rules[0].when,
        RuleMatch {
            utm_source: vec!["naver".into()],
            ..Default::default()
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn active_projects_only() {
    let db = TestDb::new().await;
    let a = project(&db.pg, "active").await;
    let _paused = project(&db.pg, "paused").await;
    let b = project(&db.pg, "active").await;
    let _deleted = project(&db.pg, "deleted").await;
    assert_eq!(s::active_project_ids(&db.pg).await.unwrap(), vec![a, b]);
}

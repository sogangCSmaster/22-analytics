//! 감사 로그와 변경의 원자성: 감사 기록이 실패하면 변경도 남지 않는다.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

/// 감사 로그를 제외한 관리 테이블 전체의 지문.
async fn snapshot(app: &TestApp) -> Vec<(String, Option<String>)> {
    let mut out = vec![];
    for t in [
        "users",
        "organizations",
        "memberships",
        "projects",
        "project_origins",
        "api_keys",
        "channel_rules",
        "attribution_settings",
        "ad_spend",
        "sessions",
    ] {
        // 테이블 이름은 위 고정 목록에서만 온다.
        let h: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT md5(string_agg(t::text, ',' ORDER BY t::text)) FROM {t} t"
        )))
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
        out.push((t.to_owned(), h));
    }
    out
}

/// 이후 `audit_log` INSERT가 모두 실패한다.
async fn break_audit(app: &TestApp) {
    for sql in [
        "CREATE FUNCTION fail_audit() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'audit unavailable'; END $$",
        "CREATE TRIGGER fail_audit BEFORE INSERT ON audit_log \
         FOR EACH ROW EXECUTE FUNCTION fail_audit()",
    ] {
        sqlx::query(sql).execute(&app.db.pg).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_audit_write_rolls_back_every_mutation() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let (org, p) = (f.org_id, f.project_id);
    app.member("free@agency.test").await;
    // 지우기·고치기 대상은 감사가 살아 있을 때 만들어 둔다.
    let r = app
        .call(
            Req::post(&format!("/v1/projects/{p}/origins"))
                .cookie(&f.analyst)
                .json(json!({ "origin": "keep.example.com" })),
        )
        .await;
    assert!(r.status.is_success(), "{}", r.text);
    let r = app
        .call(
            Req::post(&format!("/v1/projects/{p}/channel-rules"))
                .cookie(&f.analyst)
                .json(json!({ "channel": "meta", "match": { "utm_source": ["fb"] } })),
        )
        .await;
    let rule = r.body["id"].as_i64().unwrap();
    let r = app
        .call(
            Req::post(&format!("/v1/projects/{p}/api-keys"))
                .cookie(&f.admin)
                .json(json!({})),
        )
        .await;
    let key = r.body["key"]["id"].as_i64().unwrap();
    let viewer: i64 = sqlx::query_scalar("SELECT id FROM users WHERE email = 'viewer@agency.test'")
        .fetch_one(&app.db.pg)
        .await
        .unwrap();

    break_audit(&app).await;
    let before = snapshot(&app).await;

    let cases: Vec<(&str, Req)> = vec![
        (
            "user.create",
            Req::post("/v1/users").cookie(&f.root).json(json!({
                "email": "new@agency.test", "password": "initial password 1"
            })),
        ),
        (
            "org.create",
            Req::post("/v1/orgs")
                .cookie(&f.root)
                .json(json!({ "name": "새 광고주" })),
        ),
        (
            "org.update",
            Req::patch(&format!("/v1/orgs/{org}"))
                .cookie(&f.admin)
                .json(json!({ "name": "바뀐 이름" })),
        ),
        (
            "member.upsert",
            Req::post(&format!("/v1/orgs/{org}/members"))
                .cookie(&f.admin)
                .json(json!({ "email": "free@agency.test", "role": "viewer" })),
        ),
        (
            "member.remove",
            Req::delete(&format!("/v1/orgs/{org}/members/{viewer}")).cookie(&f.admin),
        ),
        (
            "project.create",
            Req::post(&format!("/v1/orgs/{org}/projects"))
                .cookie(&f.admin)
                .json(json!({ "name": "새 사이트" })),
        ),
        (
            "project.update",
            Req::patch(&format!("/v1/projects/{p}"))
                .cookie(&f.admin)
                .json(json!({ "name": "바뀐 사이트" })),
        ),
        (
            "project.settings.update",
            Req::put(&format!("/v1/projects/{p}/settings"))
                .cookie(&f.analyst)
                .json(json!({ "ecommerce": true })),
        ),
        (
            "origin.add",
            Req::post(&format!("/v1/projects/{p}/origins"))
                .cookie(&f.analyst)
                .json(json!({ "origin": "new.example.com" })),
        ),
        (
            "origin.remove",
            Req::delete(&format!("/v1/projects/{p}/origins/keep.example.com")).cookie(&f.analyst),
        ),
        (
            "channel_rule.create",
            Req::post(&format!("/v1/projects/{p}/channel-rules"))
                .cookie(&f.analyst)
                .json(json!({ "channel": "kakao", "match": { "utm_source": ["kakao"] } })),
        ),
        (
            "channel_rule.update",
            Req::patch(&format!("/v1/projects/{p}/channel-rules/{rule}"))
                .cookie(&f.analyst)
                .json(json!({ "priority": 1 })),
        ),
        (
            "channel_rule.delete",
            Req::delete(&format!("/v1/projects/{p}/channel-rules/{rule}")).cookie(&f.analyst),
        ),
        (
            "attribution.update",
            Req::put(&format!("/v1/projects/{p}/attribution-settings"))
                .cookie(&f.analyst)
                .json(json!({
                    "lookback_days": [3], "default_model": "linear", "half_life_hours": 24,
                    "include_organic": false, "include_referral": false
                })),
        ),
        (
            "ad_spend.upload",
            Req::post(&format!("/v1/projects/{p}/ad-spend"))
                .content_type("text/csv")
                .cookie(&f.analyst)
                .raw("2026-11-01,meta,,100\n"),
        ),
        (
            "api_key.create",
            Req::post(&format!("/v1/projects/{p}/api-keys"))
                .cookie(&f.admin)
                .json(json!({})),
        ),
        (
            "api_key.revoke",
            Req::delete(&format!("/v1/projects/{p}/api-keys/{key}")).cookie(&f.admin),
        ),
        (
            "auth.login",
            Req::post("/v1/auth/login")
                .json(json!({ "email": "free@agency.test", "password": PASSWORD })),
        ),
        (
            "auth.logout",
            Req::post("/v1/auth/logout").cookie(&f.viewer),
        ),
    ];
    for (action, req) in cases {
        let r = app.call(req).await;
        assert_eq!(
            r.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{action}: {}",
            r.text
        );
        assert_eq!(snapshot(&app).await, before, "{action}: 변경이 남았다");
    }
}

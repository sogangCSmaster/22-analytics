//! 프로젝트·수집 설정·허용 도메인·채널 규칙·기여 설정·스니펫.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

// ---- 프로젝트 -------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn create_project_generates_keys_and_defaults() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let r = app
        .call(
            Req::post(&format!("/v1/orgs/{}/projects", f.org_id))
                .cookie(&f.admin)
                .json(json!({ "name": "쇼핑몰", "timezone": "UTC", "currency": "USD" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let pk = r.body["public_key"].as_str().unwrap();
    assert!(analytics_core::PublicKey::parse(pk).is_ok(), "{pk}");
    assert_eq!(r.body["timezone"], "UTC");
    assert_eq!(r.body["currency"], "USD");
    assert_eq!(r.body["status"], "active");
    assert_eq!(r.body["settings_version"], 1);
    assert!(!r.text.contains("salt"), "salt는 응답에 넣지 않는다");
    let id = r.body["id"].as_i64().unwrap() as i32;

    let (salt, attr): (String, i64) = sqlx::query_as(
        "SELECT p.salt, (SELECT count(*) FROM attribution_settings a WHERE a.project_id = p.id) \
         FROM projects p WHERE p.id = $1",
    )
    .bind(id)
    .fetch_one(&app.db.pg)
    .await
    .unwrap();
    assert_eq!(salt.len(), 64);
    assert_eq!(attr, 1, "attribution_settings 행 생성");
    assert_eq!(app.audit_count("project.create").await, 2);

    // 기본 타임존·통화
    let r = app
        .call(
            Req::post(&format!("/v1/orgs/{}/projects", f.org_id))
                .cookie(&f.admin)
                .json(json!({ "name": "두 번째" })),
        )
        .await;
    assert_eq!(r.body["timezone"], "Asia/Seoul");
    assert_eq!(r.body["currency"], "KRW");
    assert_ne!(r.body["public_key"], pk);

    let list = app
        .call(Req::get(&format!("/v1/orgs/{}/projects", f.org_id)).cookie(&f.viewer))
        .await;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(list.body.as_array().unwrap().len(), 3);
    assert!(
        list.body
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["role"] == "viewer")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn create_project_validation() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/orgs/{}/projects", f.org_id);
    for body in [
        json!({ "name": "" }),
        json!({ "name": "가".repeat(201) }),
        json!({ "name": "x", "timezone": "Mars/Olympus" }),
        json!({ "name": "x", "timezone": "'; DROP TABLE projects; --" }),
        json!({ "name": "x", "currency": "won" }),
        json!({ "name": "x", "currency": "KRWX" }),
    ] {
        let r = app
            .call(Req::post(&path).cookie(&f.root).json(body.clone()))
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text);
    }
    let r = app
        .call(
            Req::post("/v1/orgs/999999/projects")
                .cookie(&f.root)
                .json(json!({ "name": "x" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "없는 광고주");
}

#[tokio::test(flavor = "multi_thread")]
async fn patch_project_and_paused_project() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/projects/{}", f.project_id);
    let r =
        app.call(Req::patch(&path).cookie(&f.admin).json(
            json!({ "name": "새 이름", "status": "paused", "timezone": "America/New_York" }),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body["name"], "새 이름");
    assert_eq!(r.body["status"], "paused");
    assert_eq!(r.body["timezone"], "America/New_York");
    assert_eq!(app.audit_count("project.update").await, 1);

    // 일시정지된 프로젝트: 수집 설정은 비활성, 관리 화면은 계속 동작
    let pk: String = sqlx::query_scalar("SELECT public_key FROM projects WHERE id = $1")
        .bind(f.project_id)
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    let cfg = analytics_pg::projects::config_by_public_key(&app.db.pg, &pk)
        .await
        .unwrap()
        .unwrap();
    assert!(!cfg.active);
    let r = app
        .call(
            Req::put(&format!("{path}/settings"))
                .cookie(&f.analyst)
                .json(json!({ "ecommerce": true })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let me = app.call(Req::get("/v1/auth/me").cookie(&f.viewer)).await;
    assert_eq!(me.body["orgs"][0]["projects"][0]["status"], "paused");

    for body in [
        json!({ "status": "deleted" }),
        json!({ "status": "ACTIVE" }),
        json!({ "name": "  " }),
        json!({ "timezone": "Not/AZone" }),
    ] {
        let r = app
            .call(Req::patch(&path).cookie(&f.admin).json(body.clone()))
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text);
    }
}

// ---- 수집 설정 ------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn settings_get_put_bumps_version_only_on_change() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/projects/{}/settings", f.project_id);

    let r = app.call(Req::get(&path).cookie(&f.viewer)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["settings_version"], 1);
    assert_eq!(r.body["settings"]["auto_pageview"], true, "기본값 채움");

    let body = json!({ "ecommerce": true, "scroll_depth": true });
    let r = app
        .call(Req::put(&path).cookie(&f.analyst).json(body.clone()))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body["settings_version"], 2);
    assert_eq!(r.body["settings"]["ecommerce"], true);

    // 같은 설정을 다시 PUT → 버전 그대로, 감사 로그도 한 번만
    let r = app
        .call(Req::put(&path).cookie(&f.analyst).json(body))
        .await;
    assert_eq!(r.body["settings_version"], 2);
    assert_eq!(app.audit_count("project.settings.update").await, 1);

    // 생략한 필드는 기본값으로 돌아간다 (PUT = 전체 교체)
    let r = app
        .call(Req::put(&path).cookie(&f.analyst).json(json!({})))
        .await;
    assert_eq!(r.body["settings_version"], 3);
    assert_eq!(r.body["settings"]["ecommerce"], false);

    // 수집 설정(Worker KV와 같은 구조)에 반영
    let pk: String = sqlx::query_scalar("SELECT public_key FROM projects WHERE id = $1")
        .bind(f.project_id)
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    let cfg = analytics_pg::projects::config_by_public_key(&app.db.pg, &pk)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cfg.settings_version, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_unknown_fields_are_ignored_not_stored() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/projects/{}/settings", f.project_id);
    let r = app
        .call(Req::put(&path).cookie(&f.analyst).json(json!({
            "ecommerce": true, "evil_field": "<script>", "nested": { "a": 1 }
        })))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT settings FROM projects WHERE id = $1")
            .bind(f.project_id)
            .fetch_one(&app.db.pg)
            .await
            .unwrap();
    assert!(stored.get("evil_field").is_none(), "{stored}");
    assert_eq!(stored["ecommerce"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_validation() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/projects/{}/settings", f.project_id);
    for body in [
        json!({ "ecommerce": "yes" }),
        json!({ "click_id_params": ["ok", ""] }),
        json!({ "click_id_params": ["has space"] }),
        json!({ "strip_query_params": ["x".repeat(101)] }),
        json!({ "capture_query_params": (0..51).map(|i| format!("p{i}")).collect::<Vec<_>>() }),
        json!([1, 2, 3]),
    ] {
        let r = app
            .call(Req::put(&path).cookie(&f.analyst).json(body.clone()))
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text);
    }
    let v: i32 = sqlx::query_scalar("SELECT settings_version FROM projects WHERE id = $1")
        .bind(f.project_id)
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    assert_eq!(v, 1);
}

// ---- 허용 도메인 ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn origins_are_normalized_and_idempotent() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/projects/{}/origins", f.project_id);

    let r = app
        .call(
            Req::post(&path)
                .cookie(&f.analyst)
                .json(json!({ "origin": " Shop.Client.CO.KR. " })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    assert_eq!(r.body["origins"], json!(["shop.client.co.kr"]));

    let r = app
        .call(
            Req::post(&path)
                .cookie(&f.analyst)
                .json(json!({ "origin": "shop.client.co.kr" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "이미 있으면 200");
    assert_eq!(r.body["origins"], json!(["shop.client.co.kr"]));
    assert_eq!(app.audit_count("origin.add").await, 1);

    // 유니코드 도메인은 punycode로 저장
    let r = app
        .call(
            Req::post(&path)
                .cookie(&f.analyst)
                .json(json!({ "origin": "한국.kr" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    for ok in ["localhost", "127.0.0.1", "xn--3e0b707e.kr"] {
        let r = app
            .call(
                Req::post(&path)
                    .cookie(&f.analyst)
                    .json(json!({ "origin": ok })),
            )
            .await;
        assert!(r.status.is_success(), "{ok}: {}", r.text);
    }
    let r = app.call(Req::get(&path).cookie(&f.viewer)).await;
    assert_eq!(
        r.body["origins"],
        json!([
            "127.0.0.1",
            "localhost",
            "shop.client.co.kr",
            "xn--3e0b707e.kr"
        ])
    );

    // 삭제 두 번 → 둘 다 204
    for _ in 0..2 {
        let r = app
            .call(Req::delete(&format!("{path}/localhost")).cookie(&f.analyst))
            .await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    }
    assert_eq!(app.audit_count("origin.remove").await, 1);
    let pk: String = sqlx::query_scalar("SELECT public_key FROM projects WHERE id = $1")
        .bind(f.project_id)
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    let cfg = analytics_pg::projects::config_by_public_key(&app.db.pg, &pk)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cfg.allowed_origins.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn origin_validation_rejects_urls_and_junk() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/projects/{}/origins", f.project_id);
    for bad in [
        "",
        "   ",
        "https://client.com",
        "client.com/path",
        "client.com:8080",
        "user@client.com",
        "*.client.com",
        "client .com",
        "client.com?x=1",
        "client.com#frag",
        "-client.com",
        "client-.com",
        "cli_ent.com",
        "com",
        "[::1]",
        "a..b.com",
        &format!("{}.com", "a".repeat(64)),
        &format!("{}com", "abc.".repeat(70)),
        "'; DROP TABLE project_origins; --",
        "<script>.com",
    ] {
        let r = app
            .call(
                Req::post(&path)
                    .cookie(&f.analyst)
                    .json(json!({ "origin": bad })),
            )
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad:?}: {}", r.text);
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM project_origins")
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

// ---- 채널 규칙 ------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn channel_rules_crud() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/projects/{}/channel-rules", f.project_id);

    let r = app
        .call(Req::post(&path).cookie(&f.analyst).json(json!({
            "channel": "naver_sa",
            "match": { "utm_source": ["naver"], "utm_medium": ["cpc"] },
            "label": "네이버 검색광고"
        })))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    assert_eq!(r.body["priority"], 100);
    let id = r.body["id"].as_i64().unwrap();
    let r = app
        .call(Req::post(&path).cookie(&f.analyst).json(json!({
            "channel": "meta", "priority": 10, "match": { "click_id_type": ["fbclid"] }
        })))
        .await;
    assert_eq!(r.status, StatusCode::CREATED);

    let list = app.call(Req::get(&path).cookie(&f.viewer)).await;
    let rules = list.body.as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["channel"], "meta", "priority 오름차순");
    assert_eq!(rules[1]["match"]["utm_source"], json!(["naver"]));

    // PATCH 두 번 → 같은 결과
    for _ in 0..2 {
        let r = app
            .call(
                Req::patch(&format!("{path}/{id}"))
                    .cookie(&f.analyst)
                    .json(json!({ "priority": 5, "label": "수정" })),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        assert_eq!(r.body["priority"], 5);
        assert_eq!(r.body["channel"], "naver_sa");
        assert_eq!(r.body["match"]["utm_medium"], json!(["cpc"]));
    }

    // 수집 설정에 반영 (프로젝트 규칙 먼저)
    let pk: String = sqlx::query_scalar("SELECT public_key FROM projects WHERE id = $1")
        .bind(f.project_id)
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    let cfg = analytics_pg::projects::config_by_public_key(&app.db.pg, &pk)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cfg.channel_rules[0].channel, "naver_sa");

    for _ in 0..2 {
        let r = app
            .call(Req::delete(&format!("{path}/{id}")).cookie(&f.analyst))
            .await;
        assert_eq!(r.status, StatusCode::NO_CONTENT);
    }
    let list = app.call(Req::get(&path).cookie(&f.viewer)).await;
    assert_eq!(list.body.as_array().unwrap().len(), 1);
    assert_eq!(app.audit_count("channel_rule.create").await, 2);
    assert_eq!(app.audit_count("channel_rule.delete").await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn channel_rule_validation() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/projects/{}/channel-rules", f.project_id);
    for body in [
        json!({ "channel": "x", "match": {} }),
        json!({ "channel": "x", "match": { "utm_source": [] } }),
        json!({ "channel": "x", "match": { "unknown_key": ["a"] } }),
        json!({ "channel": "x", "match": { "utm_source": [""] } }),
        json!({ "channel": "x", "match": { "utm_source": ["  "] } }),
        json!({ "channel": "", "match": { "utm_source": ["a"] } }),
        json!({ "channel": "가".repeat(101), "match": { "utm_source": ["a"] } }),
        json!({ "channel": "x", "match": { "utm_source": ["a".repeat(201)] } }),
        json!({ "channel": "x", "priority": -1, "match": { "utm_source": ["a"] } }),
        json!({ "channel": "x", "priority": 100001, "match": { "utm_source": ["a"] } }),
        json!({ "channel": "x" }),
    ] {
        let r = app
            .call(Req::post(&path).cookie(&f.analyst).json(body.clone()))
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text);
    }
    let r = app
        .call(Req::post(&path).cookie(&f.analyst).json(json!({
            "channel": "x", "match": { "utm_source": ["a"] }
        })))
        .await;
    let id = r.body["id"].as_i64().unwrap();
    let r = app
        .call(
            Req::patch(&format!("{path}/{id}"))
                .cookie(&f.analyst)
                .json(json!({ "match": {} })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = app
        .call(
            Req::patch(&format!("{path}/999999"))
                .cookie(&f.analyst)
                .json(json!({ "label": "x" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

// ---- 기여 설정 ------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn attribution_settings_get_put() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/projects/{}/attribution-settings", f.project_id);
    let r = app.call(Req::get(&path).cookie(&f.viewer)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(
        r.body,
        json!({
            "lookback_days": [7, 30], "default_model": "last_click", "half_life_hours": 168,
            "include_organic": true, "include_referral": true
        })
    );
    let body = json!({
        "lookback_days": [90, 1, 7, 7], "default_model": "time_decay", "half_life_hours": 2160,
        "include_organic": false, "include_referral": true
    });
    for _ in 0..2 {
        let r = app
            .call(Req::put(&path).cookie(&f.analyst).json(body.clone()))
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        assert_eq!(r.body["lookback_days"], json!([1, 7, 90]), "정렬·중복 제거");
        assert_eq!(r.body["default_model"], "time_decay");
    }
    let r = app.call(Req::get(&path).cookie(&f.viewer)).await;
    assert_eq!(r.body["half_life_hours"], 2160);
}

#[tokio::test(flavor = "multi_thread")]
async fn attribution_settings_validation() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/projects/{}/attribution-settings", f.project_id);
    let base = json!({
        "lookback_days": [7], "default_model": "last_click", "half_life_hours": 24,
        "include_organic": true, "include_referral": true
    });
    let with = |k: &str, v: serde_json::Value| {
        let mut b = base.clone();
        b[k] = v;
        b
    };
    for body in [
        with("lookback_days", json!([])),
        with("lookback_days", json!([0])),
        with("lookback_days", json!([91])),
        with("lookback_days", json!([-7])),
        with("lookback_days", json!((1..=11).collect::<Vec<_>>())),
        with("half_life_hours", json!(0)),
        with("half_life_hours", json!(2161)),
        with("default_model", json!("raw_sum")),
        with("default_model", json!("magic")),
    ] {
        let r = app
            .call(Req::put(&path).cookie(&f.analyst).json(body.clone()))
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text);
    }
    let r = app
        .call(
            Req::put(&path)
                .cookie(&f.analyst)
                .json(with("lookback_days", json!([1, 90]))),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "경계값 1, 90 허용: {}", r.text);
}

// ---- 스니펫 ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn snippet_contains_public_key_and_tracker_host() {
    let app =
        TestApp::with_config(|c| c.tracker_host = "https://collect.agency.test/".into()).await;
    let f = app.fixture().await;
    let r = app
        .call(Req::get(&format!("/v1/projects/{}/snippet", f.project_id)).cookie(&f.viewer))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let pk = r.body["public_key"].as_str().unwrap();
    let html = r.body["html"].as_str().unwrap();
    assert_eq!(r.body["tracker_host"], "https://collect.agency.test");
    assert!(
        html.contains(&format!(
            r#"<script async src="https://collect.agency.test/t.js" data-key="{pk}"></script>"#
        )),
        "{html}"
    );
    assert!(html.contains("window.at=window.at||function(){(at.q=at.q||[]).push(arguments)};"));
}

#[tokio::test(flavor = "multi_thread")]
async fn snippet_default_tracker_host() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let r = app
        .call(Req::get(&format!("/v1/projects/{}/snippet", f.project_id)).cookie(&f.viewer))
        .await;
    assert_eq!(r.body["tracker_host"], "http://localhost:8081");
}

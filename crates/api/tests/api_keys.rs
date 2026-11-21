//! S2S 비밀 키 관리 (`/v1/projects/{id}/api-keys`).

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

fn keys(p: i32) -> String {
    format!("/v1/projects/{p}/api-keys")
}

async fn create(app: &TestApp, cookie: &str, p: i32) -> Resp {
    app.call(Req::post(&keys(p)).cookie(cookie).json(json!({})))
        .await
}

/// 키를 만들고 (평문, 키 id)를 돌려준다.
async fn create_ok(app: &TestApp, cookie: &str, p: i32) -> (String, i64) {
    let r = create(app, cookie, p).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    (
        r.body["secret"].as_str().unwrap().to_owned(),
        r.body["key"]["id"].as_i64().unwrap(),
    )
}

async fn list(app: &TestApp, cookie: &str, p: i32) -> Resp {
    app.call(Req::get(&keys(p)).cookie(cookie)).await
}

async fn revoke(app: &TestApp, cookie: &str, p: i32, id: &str) -> Resp {
    app.call(Req::delete(&format!("{}/{id}", keys(p))).cookie(cookie))
        .await
}

async fn verify(app: &TestApp, secret: &str) -> bool {
    analytics_pg::api_keys::verify(&app.db.pg, secret)
        .await
        .unwrap()
        .is_some()
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_creates_key_shown_once() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let r = create(&app, &f.admin, f.project_id).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let secret = r.body["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("sk_live_"));
    let key = &r.body["key"];
    assert_eq!(key["prefix"].as_str().unwrap(), &secret[..16]);
    assert_eq!(key["scopes"], json!(["orders:write"]));
    assert!(key["last_used_at"].is_null() && key["revoked_at"].is_null());
    assert!(key["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(verify(&app, &secret).await, "발급된 키로 S2S 인증");
    assert_eq!(app.audit_count("api_key.create").await, 1);
    // 감사 로그에 평문 키가 없다
    let payloads: Vec<String> = sqlx::query_scalar("SELECT payload::text || target FROM audit_log")
        .fetch_all(&app.db.pg)
        .await
        .unwrap();
    assert!(payloads.iter().all(|p| !p.contains(&secret)));

    // 다시 만들면 다른 키
    let (secret2, _) = create_ok(&app, &f.root, f.project_id).await;
    assert_ne!(secret, secret2);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_never_contains_secret_or_hash() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let (secret, id) = create_ok(&app, &f.admin, f.project_id).await;
    assert!(verify(&app, &secret).await); // last_used_at 갱신
    let hash = analytics_pg::api_keys::hash_key(&secret);
    for cookie in [&f.viewer, &f.analyst, &f.admin, &f.root] {
        let r = list(&app, cookie, f.project_id).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        assert!(!r.text.contains(&secret) && !r.text.contains(&secret[16..]));
        assert!(!r.text.contains(&hash) && !r.text.contains("hash"));
        let items = r.body.as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], id);
        assert!(items[0]["last_used_at"].is_string());
        let fields: Vec<&String> = items[0].as_object().unwrap().keys().collect();
        assert!(fields.iter().all(|k| !k.contains("secret")), "{fields:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_admin_can_create_or_revoke() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let (secret, id) = create_ok(&app, &f.admin, f.project_id).await;
    for cookie in [&f.analyst, &f.viewer] {
        let r = create(&app, cookie, f.project_id).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text);
        let r = revoke(&app, cookie, f.project_id, &id.to_string()).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text);
    }
    assert!(verify(&app, &secret).await);
    let n = list(&app, &f.viewer, f.project_id).await;
    assert_eq!(n.body.as_array().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_is_idempotent() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let (secret, id) = create_ok(&app, &f.admin, f.project_id).await;
    for _ in 0..2 {
        let r = revoke(&app, &f.admin, f.project_id, &id.to_string()).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
        assert!(r.text.is_empty());
    }
    assert_eq!(
        app.audit_count("api_key.revoke").await,
        1,
        "실제 폐기만 기록"
    );
    assert!(!verify(&app, &secret).await);
    let r = list(&app, &f.viewer, f.project_id).await;
    assert!(r.body[0]["revoked_at"].is_string());
    // 없는 키 id
    let r = revoke(&app, &f.admin, f.project_id, "987654").await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "multi_thread")]
async fn other_projects_keys_are_untouchable() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let org_b = app.create_org(&f.root, "광고주 B").await;
    let other = app.create_project(&f.root, org_b, "사이트 B").await;
    let (secret_b, id_b) = create_ok(&app, &f.root, other).await;

    // 다른 광고주 프로젝트 자체는 404
    for r in [
        list(&app, &f.admin, other).await,
        create(&app, &f.admin, other).await,
        revoke(&app, &f.admin, other, &id_b.to_string()).await,
    ] {
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.text);
    }
    // 내 프로젝트 경로로 남의 키 id → 204지만 효과 없음
    let r = revoke(&app, &f.admin, f.project_id, &id_b.to_string()).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(
        verify(&app, &secret_b).await,
        "다른 프로젝트 키는 폐기되지 않음"
    );
    assert_eq!(app.audit_count("api_key.revoke").await, 0);
    let r = list(&app, &f.admin, f.project_id).await;
    assert_eq!(r.body, json!([]));

    // 같은 광고주의 다른 프로젝트 키도 마찬가지
    let sibling = app.create_project(&f.root, f.org_id, "사이트 A2").await;
    let (secret_s, id_s) = create_ok(&app, &f.admin, sibling).await;
    let r = revoke(&app, &f.admin, f.project_id, &id_s.to_string()).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(verify(&app, &secret_s).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_csrf_and_bad_ids() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let p = f.project_id;
    assert_eq!(
        app.call(Req::get(&keys(p))).await.status,
        StatusCode::UNAUTHORIZED
    );
    let r = app.call(Req::post(&keys(p)).json(json!({}))).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // CSRF: Origin 없음 / 다른 Origin / form Content-Type
    for req in [
        Req::post(&keys(p)).origin(None),
        Req::post(&keys(p)).origin(Some("https://evil.test")),
        Req::post(&keys(p)).content_type("application/x-www-form-urlencoded"),
        Req::delete(&format!("{}/1", keys(p))).origin(Some("https://evil.test")),
    ] {
        let r = app.call(req.cookie(&f.admin).json(json!({}))).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text);
        assert_eq!(r.error_code(), "csrf_rejected");
    }
    assert_eq!(app.audit_count("api_key.create").await, 0);
    for bad in ["abc", "-1", "1%20OR%201=1", "99999999999999999999"] {
        let r = revoke(&app, &f.admin, p, bad).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{bad}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn active_key_count_is_limited() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let mut ids = vec![];
    for _ in 0..20 {
        ids.push(create_ok(&app, &f.admin, f.project_id).await.1);
    }
    let r = create(&app, &f.admin, f.project_id).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
    assert_eq!(r.error_code(), "limit_exceeded");
    let r = revoke(&app, &f.admin, f.project_id, &ids[0].to_string()).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    create_ok(&app, &f.admin, f.project_id).await;
    let r = list(&app, &f.viewer, f.project_id).await;
    let items: &Vec<Value> = r.body.as_array().unwrap();
    assert_eq!(items.len(), 21, "폐기된 키도 목록에 남는다");
    assert!(
        items[0]["id"].as_i64().unwrap() > items[1]["id"].as_i64().unwrap(),
        "최신순"
    );
}

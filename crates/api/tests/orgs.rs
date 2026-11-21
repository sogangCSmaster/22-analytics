//! 사용자·광고주·멤버 관리.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

// ---- 사용자 ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn super_admin_creates_and_lists_users() {
    let app = TestApp::new().await;
    let root = app.super_admin().await;
    let r = app
        .call(Req::post("/v1/users").cookie(&root).json(json!({
            "email": " New.AE@Agency.test ", "password": "initial password", "name": "김에이"
        })))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    assert_eq!(r.body["email"], "new.ae@agency.test", "소문자·trim");
    assert_eq!(r.body["name"], "김에이");
    assert_eq!(r.body["is_super_admin"], false);
    assert_eq!(app.audit_count("user.create").await, 1);

    // 초기 비밀번호로 로그인 가능
    app.login("new.ae@agency.test", "initial password").await;

    let list = app.call(Req::get("/v1/users").cookie(&root)).await;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(list.body["total"], 2);
    assert_eq!(list.body["limit"], 50);
    assert_eq!(list.body["offset"], 0);

    let found = app
        .call(Req::get("/v1/users?q=%EA%B9%80").cookie(&root)) // "김"
        .await;
    assert_eq!(found.body["total"], 1, "{}", found.text);
    assert_eq!(found.body["items"][0]["name"], "김에이");
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_email_is_409_case_insensitive() {
    let app = TestApp::new().await;
    let root = app.super_admin().await;
    let body = |email: &str| json!({ "email": email, "password": "initial password" });
    let r = app
        .call(
            Req::post("/v1/users")
                .cookie(&root)
                .json(body("a@agency.test")),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = app
        .call(
            Req::post("/v1/users")
                .cookie(&root)
                .json(body("A@AGENCY.test")),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
    assert_eq!(r.error_code(), "email_taken");
}

#[tokio::test(flavor = "multi_thread")]
async fn user_input_validation() {
    let app = TestApp::new().await;
    let root = app.super_admin().await;
    for body in [
        json!({ "email": "", "password": "initial password" }),
        json!({ "email": "no-at-sign", "password": "initial password" }),
        json!({ "email": "a@b@c", "password": "initial password" }),
        json!({ "email": format!("{}@x.com", "a".repeat(250)), "password": "initial password" }),
        json!({ "email": "ok@agency.test", "password": "short" }),
        json!({ "email": "ok@agency.test", "password": "x".repeat(257) }),
        json!({ "email": "ok@agency.test", "password": "initial password", "name": "가".repeat(201) }),
    ] {
        let r = app
            .call(Req::post("/v1/users").cookie(&root).json(body.clone()))
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text);
        assert_eq!(r.error_code(), "invalid_input");
    }
    // 이름 200자(유니코드)는 허용
    let r = app
        .call(Req::post("/v1/users").cookie(&root).json(json!({
            "email": "ok@agency.test", "password": "initial password", "name": "가".repeat(200)
        })))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn pagination_bounds() {
    let app = TestApp::new().await;
    let root = app.super_admin().await;
    for i in 0..3 {
        app.user(&format!("u{i}@agency.test"), PASSWORD, false)
            .await;
    }
    let r = app
        .call(Req::get("/v1/users?limit=2&offset=0").cookie(&root))
        .await;
    assert_eq!(r.body["items"].as_array().unwrap().len(), 2);
    assert_eq!(r.body["total"], 4);
    let r = app
        .call(Req::get("/v1/users?limit=2&offset=2").cookie(&root))
        .await;
    assert_eq!(r.body["items"].as_array().unwrap().len(), 2);
    let r = app
        .call(Req::get("/v1/users?limit=200&offset=100").cookie(&root))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["items"], json!([]));

    for q in [
        "limit=0",
        "limit=201",
        "limit=-1",
        "offset=-1",
        "limit=abc",
        "offset=99999999999999999999",
    ] {
        let r = app
            .call(Req::get(&format!("/v1/users?{q}")).cookie(&root))
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{q}: {}", r.text);
        assert_eq!(r.error_code(), "invalid_input");
    }
}

// ---- 광고주 ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn org_crud() {
    let app = TestApp::new().await;
    let root = app.super_admin().await;
    let r = app
        .call(
            Req::post("/v1/orgs")
                .cookie(&root)
                .json(json!({ "name": "  광고주 🚀  ", "memo": "메모" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    assert_eq!(r.body["name"], "광고주 🚀", "앞뒤 공백 제거");
    assert_eq!(r.body["role"], "admin");
    let id = r.body["id"].as_i64().unwrap();
    assert_eq!(app.audit_count("org.create").await, 1);

    let r = app
        .call(Req::get(&format!("/v1/orgs/{id}")).cookie(&root))
        .await;
    assert_eq!(r.body["memo"], "메모");

    let r = app
        .call(
            Req::patch(&format!("/v1/orgs/{id}"))
                .cookie(&root)
                .json(json!({ "memo": "새 메모" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body["name"], "광고주 🚀", "빠진 필드는 유지");
    assert_eq!(r.body["memo"], "새 메모");
    assert_eq!(app.audit_count("org.update").await, 1);

    let r = app.call(Req::get("/v1/orgs").cookie(&root)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body.as_array().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn org_name_validation() {
    let app = TestApp::new().await;
    let root = app.super_admin().await;
    for name in [
        json!(""),
        json!("   "),
        json!("가".repeat(201)),
        json!("tab\tname"),
        json!("nul\u{0}name"),
        json!(123),
    ] {
        let r = app
            .call(
                Req::post("/v1/orgs")
                    .cookie(&root)
                    .json(json!({ "name": name })),
            )
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{name}: {}", r.text);
    }
    let r = app
        .call(
            Req::post("/v1/orgs")
                .cookie(&root)
                .json(json!({ "name": "가".repeat(200) })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let id = r.body["id"].as_i64().unwrap();
    let r = app
        .call(
            Req::patch(&format!("/v1/orgs/{id}"))
                .cookie(&root)
                .json(json!({ "name": "" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn org_list_is_scoped_to_membership() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    app.create_org(&f.root, "광고주 B").await;
    let r = app.call(Req::get("/v1/orgs").cookie(&f.viewer)).await;
    let orgs = r.body.as_array().unwrap();
    assert_eq!(orgs.len(), 1);
    assert_eq!(orgs[0]["id"], f.org_id);
    assert_eq!(orgs[0]["role"], "viewer");
    let r = app.call(Req::get("/v1/orgs").cookie(&f.root)).await;
    assert_eq!(r.body.as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn org_admin_can_update_own_org() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let r = app
        .call(
            Req::patch(&format!("/v1/orgs/{}", f.org_id))
                .cookie(&f.admin)
                .json(json!({ "name": "바뀐 이름" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body["role"], "admin");
}

// ---- 멤버 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn members_add_upsert_list_remove() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let (uid, _) = app.member("new@agency.test").await;
    let path = format!("/v1/orgs/{}/members", f.org_id);

    // org admin이 추가
    let r = app
        .call(
            Req::post(&path)
                .cookie(&f.admin)
                .json(json!({ "email": "NEW@agency.test", "role": "viewer" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body["role"], "viewer");
    assert_eq!(r.body["user_id"], uid);

    // 같은 요청을 두 번 → 멤버십 하나, 역할 갱신
    let r = app
        .call(
            Req::post(&path)
                .cookie(&f.admin)
                .json(json!({ "email": "new@agency.test", "role": "analyst" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let list = app.call(Req::get(&path).cookie(&f.admin)).await;
    assert_eq!(list.status, StatusCode::OK);
    let members = list.body.as_array().unwrap();
    assert_eq!(
        members.len(),
        4,
        "admin, analyst, viewer, new: {}",
        list.text
    );
    let m = members.iter().find(|m| m["user_id"] == uid).unwrap();
    assert_eq!(m["role"], "analyst");
    assert!(!list.text.contains("password"));

    // 제거 두 번 → 둘 다 204
    for _ in 0..2 {
        let r = app
            .call(Req::delete(&format!("{path}/{uid}")).cookie(&f.admin))
            .await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    }
    let list = app.call(Req::get(&path).cookie(&f.root)).await;
    assert_eq!(list.body.as_array().unwrap().len(), 3);
    assert_eq!(app.audit_count("member.upsert").await, 3 + 2);
    assert_eq!(
        app.audit_count("member.remove").await,
        1,
        "실제 삭제만 기록"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn add_member_validation() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/orgs/{}/members", f.org_id);
    let r = app
        .call(
            Req::post(&path)
                .cookie(&f.root)
                .json(json!({ "email": "ghost@agency.test", "role": "viewer" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.error_code(), "user_not_found");
    let r = app
        .call(
            Req::post(&path)
                .cookie(&f.root)
                .json(json!({ "email": "admin@agency.test", "role": "owner" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn removed_member_loses_access_immediately() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let uid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE email = 'analyst@agency.test'")
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    let p = format!("/v1/projects/{}", f.project_id);
    assert_eq!(
        app.call(Req::get(&p).cookie(&f.analyst)).await.status,
        StatusCode::OK
    );
    app.call(Req::delete(&format!("/v1/orgs/{}/members/{uid}", f.org_id)).cookie(&f.root))
        .await;
    assert_eq!(
        app.call(Req::get(&p).cookie(&f.analyst)).await.status,
        StatusCode::NOT_FOUND
    );
}

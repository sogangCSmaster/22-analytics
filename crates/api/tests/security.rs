//! 외부 공격: CSRF, CORS, 테넌트 격리(IDOR), 권한 상승, 인젝션, 과대 페이로드.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

// ---- CSRF -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn mutations_require_allowed_origin() {
    let app = TestApp::new().await;
    let root = app.super_admin().await;
    let body = json!({ "name": "광고주" });

    for origin in [
        None,
        Some("https://evil.example"),
        Some("http://localhost:8080.evil.example"),
        Some("null"),
        Some("http://localhost:8081"),
    ] {
        let r = app
            .call(
                Req::post("/v1/orgs")
                    .cookie(&root)
                    .origin(origin)
                    .json(body.clone()),
            )
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "origin={origin:?}");
        assert_eq!(r.error_code(), "csrf_rejected");
    }
    // 허용 목록의 다른 origin은 통과
    let r = app
        .call(
            Req::post("/v1/orgs")
                .cookie(&root)
                .origin(Some("http://127.0.0.1:8080"))
                .json(body),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM organizations")
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    assert_eq!(n, 1, "거부된 요청은 아무것도 만들지 않는다");
}

#[tokio::test(flavor = "multi_thread")]
async fn mutations_require_json_content_type() {
    let app = TestApp::new().await;
    let root = app.super_admin().await;
    for ct in [
        None,
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
        Some("multipart/form-data; boundary=x"),
    ] {
        let mut req = Req::post("/v1/orgs")
            .cookie(&root)
            .raw(r#"{"name":"광고주"}"#);
        req = match ct {
            Some(ct) => req.content_type(ct),
            None => req.no_header("content-type"),
        };
        let r = app.call(req).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "content-type={ct:?}");
        assert_eq!(r.error_code(), "csrf_rejected");
    }
    // charset 붙은 JSON은 허용
    let r = app
        .call(
            Req::post("/v1/orgs")
                .cookie(&root)
                .content_type("application/json; charset=utf-8")
                .raw(r#"{"name":"광고주"}"#),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_and_logout_are_csrf_protected() {
    let app = TestApp::new().await;
    app.user("ae@agency.test", PASSWORD, false).await;
    let r = app
        .call(
            Req::post("/v1/auth/login")
                .origin(Some("https://evil.example"))
                .json(json!({ "email": "ae@agency.test", "password": PASSWORD })),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.set_cookie().is_none());

    let cookie = app.login("ae@agency.test", PASSWORD).await;
    let r = app
        .call(
            Req::post("/v1/auth/logout")
                .cookie(&cookie)
                .origin(Some("https://evil.example")),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let me = app.call(Req::get("/v1/auth/me").cookie(&cookie)).await;
    assert_eq!(me.status, StatusCode::OK, "강제 로그아웃 CSRF 차단");
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_is_csrf_protected() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let path = format!("/v1/projects/{}/origins/localhost", f.project_id);
    app.call(
        Req::post(&format!("/v1/projects/{}/origins", f.project_id))
            .cookie(&f.root)
            .json(json!({ "origin": "localhost" })),
    )
    .await;
    let r = app
        .call(
            Req::delete(&path)
                .cookie(&f.root)
                .origin(Some("https://evil.example")),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_requests_need_no_origin() {
    let app = TestApp::new().await;
    let (_, cookie) = app.member("ae@agency.test").await;
    let r = app
        .call(Req::get("/v1/auth/me").cookie(&cookie).origin(None))
        .await;
    assert_eq!(r.status, StatusCode::OK);
}

// ---- CORS -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn cors_allows_only_dashboard_origins_with_credentials() {
    let app = TestApp::new().await;
    let pre = |origin: &str| {
        Req::new(axum::http::Method::OPTIONS, "/v1/orgs")
            .origin(Some(origin))
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "content-type")
    };
    let ok = app.call(pre(ORIGIN)).await;
    assert!(ok.status.is_success(), "{}", ok.status);
    assert_eq!(
        ok.headers["access-control-allow-origin"].to_str().unwrap(),
        ORIGIN
    );
    assert_eq!(
        ok.headers["access-control-allow-credentials"]
            .to_str()
            .unwrap(),
        "true"
    );
    let bad = app.call(pre("https://evil.example")).await;
    assert!(bad.headers.get("access-control-allow-origin").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_dashboard_origins_from_config() {
    let app = TestApp::with_config(|c| {
        c.dashboard_origins = vec!["https://analytics.agency.test".into()];
    })
    .await;
    let root = super_admin_with_origin(&app, "https://analytics.agency.test").await;
    let r = app
        .call(
            Req::post("/v1/orgs")
                .cookie(&root)
                .origin(Some("https://analytics.agency.test"))
                .json(json!({ "name": "x" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = app
        .call(
            Req::post("/v1/orgs")
                .cookie(&root)
                .json(json!({ "name": "x" })),
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::FORBIDDEN,
        "기본 origin은 더 이상 허용 안 됨"
    );
}

async fn super_admin_with_origin(app: &TestApp, origin: &str) -> String {
    app.user("root@agency.test", PASSWORD, true).await;
    let r = app
        .call(
            Req::post("/v1/auth/login")
                .origin(Some(origin))
                .json(json!({ "email": "root@agency.test", "password": PASSWORD })),
        )
        .await;
    r.session_cookie().unwrap()
}

// ---- 테넌트 격리 (IDOR) ----------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn member_of_org_a_cannot_see_org_b_project_returns_404() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let org_b = app.create_org(&f.root, "광고주 B").await;
    let project_b = app.create_project(&f.root, org_b, "사이트 B").await;
    let missing: i64 = 999_999;

    let paths = |p: i64, o: i64| {
        vec![
            format!("/v1/projects/{p}"),
            format!("/v1/projects/{p}/settings"),
            format!("/v1/projects/{p}/origins"),
            format!("/v1/projects/{p}/channel-rules"),
            format!("/v1/projects/{p}/attribution-settings"),
            format!("/v1/projects/{p}/snippet"),
            format!("/v1/projects/{p}/ad-spend?from=2026-01-01&to=2026-01-31"),
            format!("/v1/orgs/{o}"),
            format!("/v1/orgs/{o}/projects"),
            format!("/v1/orgs/{o}/members"),
        ]
    };
    for cookie in [&f.admin, &f.analyst, &f.viewer] {
        let real = paths(project_b as i64, org_b);
        let probes = paths(missing, missing);
        for (path, probe) in real.iter().zip(probes.iter()) {
            let r = app.call(Req::get(path).cookie(cookie)).await;
            assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
            // 존재하지 않는 id와 응답이 같아야 한다 (존재 여부 탐지 불가).
            let r2 = app.call(Req::get(probe).cookie(cookie)).await;
            assert_eq!(r2.status, r.status, "{probe}");
            assert_eq!(r2.text, r.text, "{probe}");
        }
        // 변경 시도도 404
        let r = app
            .call(
                Req::patch(&format!("/v1/projects/{project_b}"))
                    .cookie(cookie)
                    .json(json!({ "name": "탈취" })),
            )
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
        let r = app
            .call(
                Req::put(&format!("/v1/projects/{project_b}/settings"))
                    .cookie(cookie)
                    .json(json!({ "ecommerce": true })),
            )
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
        let r = app
            .call(
                Req::post(&format!("/v1/orgs/{org_b}/projects"))
                    .cookie(cookie)
                    .json(json!({ "name": "침투" })),
            )
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
    }
    let name: String = sqlx::query_scalar("SELECT name FROM projects WHERE id = $1")
        .bind(project_b)
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    assert_eq!(name, "사이트 B");
}

#[tokio::test(flavor = "multi_thread")]
async fn channel_rule_of_other_project_is_404() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let other = app.create_project(&f.root, f.org_id, "사이트 2").await;
    let r = app
        .call(
            Req::post(&format!("/v1/projects/{other}/channel-rules"))
                .cookie(&f.root)
                .json(json!({ "channel": "naver_sa", "match": { "utm_source": ["naver"] } })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let rule_id = r.body["id"].as_i64().unwrap();
    // 자기 프로젝트 경로로 남의 규칙 id를 건드리면 404
    let path = format!("/v1/projects/{}/channel-rules/{rule_id}", f.project_id);
    let r = app
        .call(
            Req::patch(&path)
                .cookie(&f.root)
                .json(json!({ "channel": "hijack" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // DELETE는 멱등이라 없는 id와 똑같이 204를 주되, 남의 규칙은 지워지지 않아야 한다.
    let r = app.call(Req::delete(&path).cookie(&f.root)).await;
    let missing = app
        .call(
            Req::delete(&format!(
                "/v1/projects/{}/channel-rules/999999",
                f.project_id
            ))
            .cookie(&f.root),
        )
        .await;
    assert_eq!(r.status, missing.status);
    assert_eq!(r.text, missing.text);
    let still: String = sqlx::query_scalar("SELECT channel FROM channel_rules WHERE id = $1")
        .bind(rule_id)
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    assert_eq!(still, "naver_sa");
}

#[tokio::test(flavor = "multi_thread")]
async fn non_numeric_and_overflow_ids_are_404() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    for path in [
        "/v1/projects/abc",
        "/v1/projects/1%20OR%201=1",
        "/v1/projects/99999999999999999999",
        "/v1/projects/-1",
        "/v1/orgs/abc",
        "/v1/orgs/99999999999999999999999",
    ] {
        let r = app.call(Req::get(path).cookie(&f.analyst)).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
    }
}

// ---- 권한 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn unauthenticated_requests_are_401() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    for path in [
        "/v1/users".to_owned(),
        "/v1/orgs".to_owned(),
        format!("/v1/orgs/{}", f.org_id),
        format!("/v1/projects/{}", f.project_id),
        format!("/v1/projects/{}/settings", f.project_id),
    ] {
        let r = app.call(Req::get(&path)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}");
    }
    let r = app
        .call(Req::post("/v1/orgs").json(json!({ "name": "x" })))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn viewer_cannot_mutate() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let p = f.project_id;
    let o = f.org_id;
    let attempts = vec![
        Req::patch(&format!("/v1/projects/{p}")).json(json!({ "name": "x" })),
        Req::put(&format!("/v1/projects/{p}/settings")).json(json!({ "ecommerce": true })),
        Req::post(&format!("/v1/projects/{p}/origins")).json(json!({ "origin": "client.com" })),
        Req::delete(&format!("/v1/projects/{p}/origins/client.com")),
        Req::post(&format!("/v1/projects/{p}/channel-rules"))
            .json(json!({ "channel": "x", "match": { "utm_source": ["x"] } })),
        Req::put(&format!("/v1/projects/{p}/attribution-settings")).json(json!({
            "lookback_days": [7], "default_model": "last_click", "half_life_hours": 24,
            "include_organic": true, "include_referral": true
        })),
        Req::post(&format!("/v1/projects/{p}/ad-spend"))
            .content_type("text/csv")
            .raw("2026-01-01,meta,,1000\n"),
        Req::patch(&format!("/v1/orgs/{o}")).json(json!({ "name": "x" })),
        Req::post(&format!("/v1/orgs/{o}/projects")).json(json!({ "name": "x" })),
        Req::post(&format!("/v1/orgs/{o}/members"))
            .json(json!({ "email": "viewer@agency.test", "role": "admin" })),
    ];
    for req in attempts {
        let r = app.call(req.cookie(&f.viewer)).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text);
        assert_eq!(r.error_code(), "forbidden");
    }
    // 읽기는 가능
    for path in [
        format!("/v1/projects/{p}"),
        format!("/v1/projects/{p}/settings"),
        format!("/v1/projects/{p}/origins"),
        format!("/v1/projects/{p}/channel-rules"),
        format!("/v1/projects/{p}/attribution-settings"),
        format!("/v1/projects/{p}/snippet"),
        format!("/v1/orgs/{o}"),
        format!("/v1/orgs/{o}/projects"),
    ] {
        let r = app.call(Req::get(&path).cookie(&f.viewer)).await;
        assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.text);
    }
    // 역할이 그대로인지 (자기 승격 실패)
    let role: String = sqlx::query_scalar(
        "SELECT m.role FROM memberships m JOIN users u ON u.id = m.user_id WHERE u.email = 'viewer@agency.test'",
    )
    .fetch_one(&app.db.pg)
    .await
    .unwrap();
    assert_eq!(role, "viewer");
}

#[tokio::test(flavor = "multi_thread")]
async fn analyst_cannot_manage_org_or_members() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let o = f.org_id;
    for req in [
        Req::patch(&format!("/v1/orgs/{o}")).json(json!({ "name": "x" })),
        Req::post(&format!("/v1/orgs/{o}/members"))
            .json(json!({ "email": "analyst@agency.test", "role": "admin" })),
        Req::post(&format!("/v1/orgs/{o}/projects")).json(json!({ "name": "x" })),
        Req::patch(&format!("/v1/projects/{}", f.project_id)).json(json!({ "status": "paused" })),
    ] {
        let r = app.call(req.cookie(&f.analyst)).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text);
    }
    let r = app
        .call(Req::get(&format!("/v1/orgs/{o}/members")).cookie(&f.analyst))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread")]
async fn non_super_admin_cannot_create_orgs_or_users() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    for cookie in [&f.admin, &f.analyst, &f.viewer] {
        let r = app
            .call(
                Req::post("/v1/orgs")
                    .cookie(cookie)
                    .json(json!({ "name": "몰래" })),
            )
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        let r = app
            .call(Req::post("/v1/users").cookie(cookie).json(json!({
                "email": "evil@agency.test", "password": "long enough pw", "is_super_admin": true
            })))
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        let r = app.call(Req::get("/v1/users").cookie(cookie)).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = 'evil@agency.test'")
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn org_admin_cannot_touch_other_org_members() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let org_b = app.create_org(&f.root, "광고주 B").await;
    let r = app
        .call(
            Req::post(&format!("/v1/orgs/{org_b}/members"))
                .cookie(&f.admin)
                .json(json!({ "email": "admin@agency.test", "role": "admin" })),
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::NOT_FOUND,
        "자기 자신을 남의 광고주에 추가 불가"
    );
}

// ---- 인젝션·과대 페이로드 ---------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn sql_injection_strings_are_stored_literally() {
    let app = TestApp::new().await;
    let root = app.super_admin().await;
    let evil = "'; DROP TABLE projects; --";
    let org = app.create_org(&root, evil).await;
    let p = app.create_project(&root, org, "x' OR '1'='1").await;
    let r = app
        .call(Req::get(&format!("/v1/orgs/{org}")).cookie(&root))
        .await;
    assert_eq!(r.body["name"], evil);
    let r = app
        .call(Req::get(&format!("/v1/projects/{p}")).cookie(&root))
        .await;
    assert_eq!(r.body["name"], "x' OR '1'='1");

    // 검색어 인젝션과 LIKE 와일드카드
    for q in ["' OR 1=1 --", "%", "_", "\\", "%25"] {
        let r = app
            .call(Req::get(&format!("/v1/users?q={}", urlencode(q))).cookie(&root))
            .await;
        assert_eq!(r.status, StatusCode::OK, "{q}: {}", r.text);
        assert_eq!(
            r.body["total"], 0,
            "q={q}는 아무것도 맞지 않아야 함: {}",
            r.text
        );
    }
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_json_body_is_413() {
    let app = TestApp::new().await;
    let root = app.super_admin().await;
    let big = "x".repeat(2 * 1024 * 1024);
    let r = app
        .call(
            Req::post("/v1/orgs")
                .cookie(&root)
                .json(json!({ "name": "a", "memo": big })),
        )
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.text);
    let r = app
        .call(Req::post("/v1/auth/login").json(json!({ "email": big, "password": "x" })))
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test(flavor = "multi_thread")]
async fn health_endpoints_still_work() {
    let app = TestApp::new().await;
    let r = app.call(Req::get("/healthz")).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = app.call(Req::get("/readyz")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
}

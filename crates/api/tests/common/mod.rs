//! API 통합 테스트 공용 도구.
//!
//! 실제 포트를 열지 않고 `build_router`로 만든 Router를 `tower::ServiceExt::oneshot`으로 호출한다.
//! DB는 테스트마다 격리된 [`TestDb`]를 쓴다.
#![allow(dead_code)]

use analytics_api::{ApiConfig, AppState, build_router};
use analytics_testkit::TestDb;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

pub const ORIGIN: &str = "http://localhost:8080";
pub const PASSWORD: &str = "correct horse battery";

pub struct TestApp {
    pub db: TestDb,
    pub router: Router,
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub text: String,
    pub body: Value,
}

impl Resp {
    /// `Set-Cookie`에서 `at_session=...` 부분만 (속성 제외).
    pub fn session_cookie(&self) -> Option<String> {
        self.set_cookie()
            .map(|c| c.split(';').next().unwrap().trim().to_owned())
    }

    pub fn set_cookie(&self) -> Option<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with("at_session="))
            .map(str::to_owned)
    }

    pub fn error_code(&self) -> &str {
        self.body["error"].as_str().unwrap_or("")
    }
}

pub struct Req {
    method: Method,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    origin: Option<String>,
}

impl Req {
    pub fn new(method: Method, path: &str) -> Self {
        Self {
            method,
            path: path.to_owned(),
            headers: vec![],
            body: vec![],
            origin: Some(ORIGIN.to_owned()),
        }
    }
    pub fn get(path: &str) -> Self {
        Self::new(Method::GET, path)
    }
    pub fn post(path: &str) -> Self {
        Self::new(Method::POST, path).content_type("application/json")
    }
    pub fn put(path: &str) -> Self {
        Self::new(Method::PUT, path).content_type("application/json")
    }
    pub fn patch(path: &str) -> Self {
        Self::new(Method::PATCH, path).content_type("application/json")
    }
    pub fn delete(path: &str) -> Self {
        Self::new(Method::DELETE, path).content_type("application/json")
    }
    pub fn cookie(self, cookie: &str) -> Self {
        self.header("cookie", cookie)
    }
    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.retain(|(hk, _)| !hk.eq_ignore_ascii_case(k));
        self.headers.push((k.to_owned(), v.to_owned()));
        self
    }
    pub fn no_header(mut self, k: &str) -> Self {
        self.headers.retain(|(hk, _)| !hk.eq_ignore_ascii_case(k));
        self
    }
    pub fn content_type(self, ct: &str) -> Self {
        self.header("content-type", ct)
    }
    pub fn origin(mut self, origin: Option<&str>) -> Self {
        self.origin = origin.map(str::to_owned);
        self
    }
    pub fn json(mut self, v: Value) -> Self {
        self.body = serde_json::to_vec(&v).unwrap();
        self
    }
    pub fn raw(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }
}

impl TestApp {
    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    pub async fn with_config(f: impl FnOnce(&mut ApiConfig)) -> Self {
        let db = TestDb::new().await;
        let mut config = ApiConfig::default();
        f(&mut config);
        let state = AppState::new(db.pg.clone(), db.ch.clone(), config);
        let router = build_router(state);
        Self { db, router }
    }

    pub async fn call(&self, req: Req) -> Resp {
        let mut b = Request::builder().method(req.method).uri(&req.path);
        if let Some(o) = &req.origin {
            b = b.header("origin", o);
        }
        for (k, v) in &req.headers {
            b = b.header(k, v);
        }
        let request = b.body(Body::from(req.body)).unwrap();
        let res = self.router.clone().oneshot(request).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Resp {
            status,
            headers,
            text,
            body,
        }
    }

    /// DB에 직접 사용자를 만든다 (API 우회, 테스트 준비용).
    pub async fn user(&self, email: &str, password: &str, super_admin: bool) -> i64 {
        let hash = analytics_pg::users::hash_password(password).unwrap();
        analytics_pg::users::create(
            &self.db.pg,
            &analytics_pg::users::NewUser {
                email: email.to_owned(),
                password_hash: hash,
                name: String::new(),
                is_super_admin: super_admin,
            },
        )
        .await
        .unwrap()
        .id
    }

    pub async fn login_resp(&self, email: &str, password: &str) -> Resp {
        self.call(Req::post("/v1/auth/login").json(json!({ "email": email, "password": password })))
            .await
    }

    /// 로그인하고 `at_session=...` 쿠키 문자열을 돌려준다.
    pub async fn login(&self, email: &str, password: &str) -> String {
        let r = self.login_resp(email, password).await;
        assert_eq!(r.status, StatusCode::OK, "로그인 실패: {}", r.text);
        r.session_cookie().expect("세션 쿠키")
    }

    /// 슈퍼 관리자를 만들고 로그인한다.
    pub async fn super_admin(&self) -> String {
        self.user("root@agency.test", PASSWORD, true).await;
        self.login("root@agency.test", PASSWORD).await
    }

    /// 일반 사용자를 만들고 로그인한다.
    pub async fn member(&self, email: &str) -> (i64, String) {
        let id = self.user(email, PASSWORD, false).await;
        (id, self.login(email, PASSWORD).await)
    }

    pub async fn create_org(&self, cookie: &str, name: &str) -> i64 {
        let r = self
            .call(
                Req::post("/v1/orgs")
                    .cookie(cookie)
                    .json(json!({ "name": name })),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
        r.body["id"].as_i64().unwrap()
    }

    pub async fn create_project(&self, cookie: &str, org_id: i64, name: &str) -> i32 {
        let r = self
            .call(
                Req::post(&format!("/v1/orgs/{org_id}/projects"))
                    .cookie(cookie)
                    .json(json!({ "name": name })),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
        r.body["id"].as_i64().unwrap() as i32
    }

    pub async fn add_member(&self, cookie: &str, org_id: i64, email: &str, role: &str) {
        let r = self
            .call(
                Req::post(&format!("/v1/orgs/{org_id}/members"))
                    .cookie(cookie)
                    .json(json!({ "email": email, "role": role })),
            )
            .await;
        assert!(r.status.is_success(), "멤버 추가 실패: {}", r.text);
    }

    /// 슈퍼 관리자 + 광고주 + 프로젝트 1개 + 역할별 멤버 세 명.
    pub async fn fixture(&self) -> Fixture {
        let root = self.super_admin().await;
        let org_id = self.create_org(&root, "광고주 A").await;
        let project_id = self.create_project(&root, org_id, "사이트 A").await;
        let mut cookies = vec![];
        for role in ["admin", "analyst", "viewer"] {
            let email = format!("{role}@agency.test");
            let (_, c) = self.member(&email).await;
            self.add_member(&root, org_id, &email, role).await;
            cookies.push(c);
        }
        let viewer = cookies.pop().unwrap();
        let analyst = cookies.pop().unwrap();
        let admin = cookies.pop().unwrap();
        Fixture {
            root,
            admin,
            analyst,
            viewer,
            org_id,
            project_id,
        }
    }

    pub async fn audit_count(&self, action: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
            .bind(action)
            .fetch_one(&self.db.pg)
            .await
            .unwrap()
    }
}

pub struct Fixture {
    pub root: String,
    pub admin: String,
    pub analyst: String,
    pub viewer: String,
    pub org_id: i64,
    pub project_id: i32,
}

//! 로그인·세션·로그아웃·로그인 rate limit.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;
use std::time::{Duration, Instant};

// ---- 해피 케이스 ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn login_sets_secure_session_cookie_and_me_works() {
    let app = TestApp::new().await;
    app.user("ae@agency.test", PASSWORD, false).await;

    let r = app.login_resp("ae@agency.test", PASSWORD).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body["user"]["email"], "ae@agency.test");

    let sc = r.set_cookie().expect("Set-Cookie");
    assert!(sc.contains("HttpOnly"), "{sc}");
    assert!(sc.contains("SameSite=Lax"), "{sc}");
    assert!(sc.contains("Path=/"), "{sc}");
    assert!(sc.contains("Max-Age=1209600"), "14일: {sc}");
    assert!(!sc.contains("Secure"), "기본값 COOKIE_SECURE=false: {sc}");

    let token = r.session_cookie().unwrap();
    let value = token.trim_start_matches("at_session=");
    assert_eq!(value.len(), 64, "32바이트 hex 토큰");

    // DB에는 토큰이 아니라 sha256(token)이 저장된다.
    let stored: Vec<String> = sqlx::query_scalar("SELECT id FROM sessions")
        .fetch_all(&app.db.pg)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_ne!(stored[0], value);
    assert_eq!(stored[0].len(), 64);

    let me = app.call(Req::get("/v1/auth/me").cookie(&token)).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text);
    assert_eq!(me.body["user"]["email"], "ae@agency.test");
    assert_eq!(me.body["orgs"], json!([]));

    let last_login: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT last_login_at FROM users WHERE email = 'ae@agency.test'")
            .fetch_one(&app.db.pg)
            .await
            .unwrap();
    assert!(last_login.is_some(), "last_login_at 기록");
}

#[tokio::test(flavor = "multi_thread")]
async fn secure_flag_when_configured() {
    let app = TestApp::with_config(|c| c.cookie_secure = true).await;
    app.user("ae@agency.test", PASSWORD, false).await;
    let r = app.login_resp("ae@agency.test", PASSWORD).await;
    let sc = r.set_cookie().unwrap();
    // __Host- 접두사: Secure + Path=/ + Domain 없음을 브라우저가 강제한다 (하위 도메인이 덮어쓰지 못함).
    assert!(sc.starts_with("__Host-at_session="), "{sc}");
    assert!(sc.contains("Secure") && sc.contains("Path=/"), "{sc}");
    assert!(!sc.to_ascii_lowercase().contains("domain="), "{sc}");

    let cookie = r.session_cookie().unwrap();
    let token = cookie.trim_start_matches("__Host-at_session=");
    let me = app.call(Req::get("/v1/auth/me").cookie(&cookie)).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text);
    // Secure 모드에서는 접두사 없는 이름(하위 도메인·http에서 심을 수 있는 쿠키)을 받지 않는다.
    let plain = app
        .call(Req::get("/v1/auth/me").cookie(&format!("at_session={token}")))
        .await;
    assert_eq!(plain.status, StatusCode::UNAUTHORIZED);

    let out = app
        .call(Req::post("/v1/auth/logout").cookie(&cookie))
        .await;
    let cleared = out.set_cookie().unwrap();
    assert!(
        cleared.starts_with("__Host-at_session=;") && cleared.contains("Max-Age=0"),
        "{cleared}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn conflicting_duplicate_session_cookies_are_rejected() {
    let app = TestApp::new().await;
    let (_, a) = app.member("a@agency.test").await;
    let (_, b) = app.member("b@agency.test").await;
    // 같은 이름의 쿠키가 다른 값으로 두 번 오면(쿠키 심기 공격) 어느 쪽도 믿지 않는다.
    let r = app
        .call(Req::get("/v1/auth/me").cookie(&format!("{a}; {b}")))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // 같은 값이 반복되는 것은 괜찮다.
    let r = app
        .call(Req::get("/v1/auth/me").cookie(&format!("{a}; {a}")))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["user"]["email"], "a@agency.test");
}

#[tokio::test(flavor = "multi_thread")]
async fn email_is_case_insensitive_and_trimmed() {
    let app = TestApp::new().await;
    app.user("ae@agency.test", PASSWORD, false).await;
    let r = app.login_resp("  AE@Agency.TEST ", PASSWORD).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn me_lists_orgs_and_projects_with_role() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let other_org = app.create_org(&f.root, "광고주 B").await;
    app.create_project(&f.root, other_org, "사이트 B").await;

    let me = app.call(Req::get("/v1/auth/me").cookie(&f.analyst)).await;
    assert_eq!(me.status, StatusCode::OK);
    let orgs = me.body["orgs"].as_array().unwrap();
    assert_eq!(orgs.len(), 1, "자기 광고주만: {}", me.text);
    assert_eq!(orgs[0]["id"], f.org_id);
    assert_eq!(orgs[0]["role"], "analyst");
    assert_eq!(orgs[0]["projects"][0]["id"], f.project_id);
    assert_eq!(orgs[0]["projects"][0]["status"], "active");

    // 슈퍼 관리자는 모든 광고주를 admin으로 본다.
    let me = app.call(Req::get("/v1/auth/me").cookie(&f.root)).await;
    let orgs = me.body["orgs"].as_array().unwrap();
    assert_eq!(orgs.len(), 2);
    assert!(orgs.iter().all(|o| o["role"] == "admin"));
    assert_eq!(me.body["user"]["is_super_admin"], true);
}

// ---- 멱등성 ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn logout_twice_is_ok_and_invalidates_session() {
    let app = TestApp::new().await;
    let (_, cookie) = app.member("ae@agency.test").await;

    for _ in 0..2 {
        let r = app.call(Req::post("/v1/auth/logout").cookie(&cookie)).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
        let sc = r.set_cookie().expect("쿠키 삭제");
        assert!(sc.contains("Max-Age=0"), "{sc}");
    }
    let me = app.call(Req::get("/v1/auth/me").cookie(&cookie)).await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_without_session_is_noop() {
    let app = TestApp::new().await;
    let r = app.call(Req::post("/v1/auth/logout")).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_only_kills_own_session() {
    let app = TestApp::new().await;
    app.user("ae@agency.test", PASSWORD, false).await;
    let a = app.login("ae@agency.test", PASSWORD).await;
    let b = app.login("ae@agency.test", PASSWORD).await;
    assert_ne!(a, b);
    app.call(Req::post("/v1/auth/logout").cookie(&a)).await;
    let me = app.call(Req::get("/v1/auth/me").cookie(&b)).await;
    assert_eq!(me.status, StatusCode::OK, "다른 기기 세션은 유지");
}

// ---- 경계값·희귀 케이스 ----------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn me_without_cookie_is_401() {
    let app = TestApp::new().await;
    let r = app.call(Req::get("/v1/auth/me")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.error_code(), "unauthorized");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_expiring_exactly_now_is_rejected() {
    let app = TestApp::new().await;
    let (_, cookie) = app.member("ae@agency.test").await;
    sqlx::query("UPDATE sessions SET expires_at = now()")
        .execute(&app.db.pg)
        .await
        .unwrap();
    let r = app.call(Req::get("/v1/auth/me").cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_session_is_rejected() {
    let app = TestApp::new().await;
    let (_, cookie) = app.member("ae@agency.test").await;
    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 second'")
        .execute(&app.db.pg)
        .await
        .unwrap();
    let r = app.call(Req::get("/v1/auth/me").cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn session_near_expiry_is_extended() {
    let app = TestApp::new().await;
    let (_, cookie) = app.member("ae@agency.test").await;
    sqlx::query("UPDATE sessions SET expires_at = now() + interval '1 day'")
        .execute(&app.db.pg)
        .await
        .unwrap();
    let r = app.call(Req::get("/v1/auth/me").cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::OK);
    // DB만 연장하면 브라우저 쿠키(Max-Age)는 로그인 14일 뒤에 사라진다: 쿠키도 다시 보낸다.
    let sc = r.set_cookie().expect("연장 시 Set-Cookie");
    assert!(sc.starts_with(&format!("{cookie};")), "{sc}");
    assert!(sc.contains("Max-Age=1209600"), "{sc}");
    let again = app.call(Req::get("/v1/auth/me").cookie(&cookie)).await;
    assert!(
        again.set_cookie().is_none(),
        "연장이 필요 없으면 쿠키를 다시 보내지 않는다"
    );
    let extended: bool =
        sqlx::query_scalar("SELECT expires_at > now() + interval '13 days' FROM sessions")
            .fetch_one(&app.db.pg)
            .await
            .unwrap();
    assert!(extended, "남은 기간이 7일 미만이면 14일로 연장");
}

#[tokio::test(flavor = "multi_thread")]
async fn deleted_user_session_is_dead() {
    let app = TestApp::new().await;
    let (id, cookie) = app.member("ae@agency.test").await;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(&app.db.pg)
        .await
        .unwrap();
    let r = app.call(Req::get("/v1/auth/me").cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_rejects_bad_input() {
    let app = TestApp::new().await;
    // 필드 누락
    let r = app
        .call(Req::post("/v1/auth/login").json(json!({ "email": "a@b.c" })))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // 깨진 JSON
    let r = app.call(Req::post("/v1/auth/login").raw("{not json")).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // 매우 긴 비밀번호도 500 없이 401
    let r = app.login_resp("a@b.c", &"x".repeat(5000)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_credentials_are_rejected_without_touching_the_limiter() {
    let app = TestApp::with_config(|c| {
        c.trust_proxy_headers = true;
        c.login_limit.max_per_ip = 2;
    })
    .await;
    app.user("ae@agency.test", PASSWORD, false).await;
    let normal = app
        .login_resp("nobody@agency.test", "wrong password!")
        .await;
    for (email, password) in [
        (
            format!("{}@agency.test", "a".repeat(300)),
            "wrong password!".to_owned(),
        ),
        ("ae@agency.test".to_owned(), "x".repeat(257)),
        (
            format!("{}@x.test", "가".repeat(100)),
            "wrong password!".to_owned(),
        ),
    ] {
        for _ in 0..5 {
            let r = app
                .call(
                    Req::post("/v1/auth/login")
                        .header("cf-connecting-ip", "203.0.113.9")
                        .json(json!({ "email": email, "password": password })),
                )
                .await;
            assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.text);
            assert_eq!(r.text, normal.text, "일반 실패와 같은 본문");
        }
    }
    // 길이 초과 요청은 IP 제한(2회)을 쓰지 않았다.
    let r = app
        .call(
            Req::post("/v1/auth/login")
                .header("cf-connecting-ip", "203.0.113.9")
                .json(json!({ "email": "ae@agency.test", "password": PASSWORD })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
}

// ---- 외부 공격 ------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn unknown_email_and_wrong_password_look_identical() {
    let app = TestApp::with_config(|c| {
        c.login_limit.max_per_email = 1000;
        c.login_limit.max_per_ip = 1000;
    })
    .await;
    app.user("ae@agency.test", PASSWORD, false).await;

    let wrong = app.login_resp("ae@agency.test", "wrong password!").await;
    let unknown = app
        .login_resp("nobody@agency.test", "wrong password!")
        .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.status, wrong.status);
    assert_eq!(unknown.text, wrong.text, "같은 오류 본문");
    assert_eq!(wrong.error_code(), "invalid_credentials");
    assert!(wrong.set_cookie().is_none());

    // 응답 시간: 없는 이메일도 argon2 검증을 한 번 수행해야 한다.
    async fn median(app: &TestApp, email: &str) -> Duration {
        let mut v = vec![];
        for _ in 0..5 {
            let t = Instant::now();
            app.login_resp(email, "wrong password!").await;
            v.push(t.elapsed());
        }
        v.sort();
        v[2]
    }
    let t_wrong = median(&app, "ae@agency.test").await;
    let t_unknown = median(&app, "nobody@agency.test").await;
    assert!(
        t_unknown * 3 >= t_wrong,
        "없는 이메일 응답이 너무 빠름: unknown={t_unknown:?} wrong={t_wrong:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn brute_force_on_one_email_is_rate_limited() {
    let app = TestApp::with_config(|c| c.trust_proxy_headers = true).await;
    app.user("ae@agency.test", PASSWORD, false).await;
    for i in 0..5 {
        // 매번 다른 IP: 이메일 기준 제한이 걸려야 한다.
        let r = app
            .call(
                Req::post("/v1/auth/login")
                    .header("x-forwarded-for", &format!("10.0.0.{i}, 172.16.0.1"))
                    .json(json!({ "email": "ae@agency.test", "password": "nope nope" })),
            )
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "시도 {i}");
    }
    // 실패했던 IP에서는 올바른 비밀번호여도 429
    let r = app
        .call(
            Req::post("/v1/auth/login")
                .header("x-forwarded-for", "10.0.0.3")
                .json(json!({ "email": "AE@agency.test", "password": PASSWORD })),
        )
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.text);
    assert_eq!(r.error_code(), "too_many_requests");
    assert!(r.headers.get("retry-after").is_some());
    assert!(r.set_cookie().is_none());
    // IP를 모르면 잠금 예외를 적용하지 않는다.
    let r = app
        .call(
            Req::post("/v1/auth/login")
                .json(json!({ "email": "ae@agency.test", "password": PASSWORD })),
        )
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn locked_account_still_lets_owner_in_from_clean_ip_within_budget() {
    let app = TestApp::with_config(|c| {
        c.trust_proxy_headers = true;
        c.login_limit.max_per_email = 3;
        c.login_limit.max_bypass_per_email = 2;
    })
    .await;
    app.user("ae@agency.test", PASSWORD, false).await;
    let attempt = |ip: &str, password: &str| {
        Req::post("/v1/auth/login")
            .header("cf-connecting-ip", ip)
            .json(json!({ "email": "ae@agency.test", "password": password }))
    };
    // 공격자가 계정을 잠근다.
    for i in 0..3 {
        let r = app
            .call(attempt(&format!("198.51.100.{i}"), "guess guess"))
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    let r = app.call(attempt("198.51.100.0", PASSWORD)).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    // 이 계정에 실패한 적 없는 IP의 정상 사용자는 로그인된다.
    let r = app.call(attempt("192.0.2.10", PASSWORD)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);

    // 다시 잠근 뒤, IP를 바꿔 가며 예외를 쓰는 공격자는 계정당 예산(2회)까지만 시도한다.
    for i in 0..3 {
        app.call(attempt(&format!("198.51.100.{}", 10 + i), "guess guess"))
            .await;
    }
    let mut statuses = vec![];
    for i in 0..4 {
        let r = app
            .call(attempt(&format!("203.0.113.{}", 50 + i), "guess guess"))
            .await;
        statuses.push(r.status);
    }
    assert_eq!(
        statuses,
        [
            StatusCode::UNAUTHORIZED,
            StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::TOO_MANY_REQUESTS,
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_attempts_cannot_exceed_the_email_limit() {
    let app = TestApp::with_config(|c| c.login_limit.max_per_email = 5).await;
    app.user("ae@agency.test", PASSWORD, false).await;
    // 결과가 기록되기 전에 동시에 보내도 상한 이상은 비밀번호 검증까지 가지 못한다.
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..30 {
        set.spawn(call_router(
            app.router.clone(),
            Req::post("/v1/auth/login")
                .json(json!({ "email": "ae@agency.test", "password": "wrong password!" })),
        ));
    }
    let mut unauthorized = 0;
    while let Some(r) = set.join_next().await {
        let r = r.unwrap();
        match r.status {
            StatusCode::UNAUTHORIZED => unauthorized += 1,
            StatusCode::TOO_MANY_REQUESTS => {}
            s => panic!("{s}: {}", r.text),
        }
    }
    assert!(unauthorized <= 5, "검증까지 간 시도 {unauthorized}회");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_headers_are_ignored_unless_trusted() {
    let app = TestApp::with_config(|c| c.login_limit.max_per_ip = 3).await;
    app.user("ae@agency.test", PASSWORD, false).await;
    // 헤더를 매번 바꿔도 소켓 주소가 같으면 같은 클라이언트다.
    for i in 0..3 {
        let r = app
            .call(
                Req::post("/v1/auth/login")
                    .peer("192.0.2.1")
                    .header("cf-connecting-ip", &format!("203.0.113.{i}"))
                    .header("x-forwarded-for", &format!("10.1.0.{i}"))
                    .json(json!({ "email": format!("u{i}@agency.test"), "password": "x" })),
            )
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    let blocked = app
        .call(
            Req::post("/v1/auth/login")
                .peer("192.0.2.1")
                .header("cf-connecting-ip", "203.0.113.200")
                .json(json!({ "email": "ae@agency.test", "password": PASSWORD })),
        )
        .await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    let ok = app
        .call(
            Req::post("/v1/auth/login")
                .peer("192.0.2.2")
                .header("cf-connecting-ip", "203.0.113.7")
                .json(json!({ "email": "ae@agency.test", "password": PASSWORD })),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text);
    // 감사 로그에는 실제로 쓴 값과 그 출처를 남긴다 (헤더 값을 IP로 기록하지 않는다).
    let p = app.audit_payloads("auth.login").await;
    assert_eq!(p.last().unwrap(), &json!({ "peer_ip": "192.0.2.2" }));
}

#[tokio::test(flavor = "multi_thread")]
async fn trusted_proxy_header_ip_is_labelled_as_client_supplied() {
    let app = TestApp::with_config(|c| c.trust_proxy_headers = true).await;
    app.user("ae@agency.test", PASSWORD, false).await;
    let r = app
        .call(
            Req::post("/v1/auth/login")
                .peer("192.0.2.1")
                .header("cf-connecting-ip", "203.0.113.7")
                .json(json!({ "email": "ae@agency.test", "password": PASSWORD })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let p = app.audit_payloads("auth.login").await;
    assert_eq!(
        p.last().unwrap(),
        &json!({ "client_supplied_ip": "203.0.113.7", "peer_ip": "192.0.2.1" })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn brute_force_from_one_ip_is_rate_limited() {
    let app = TestApp::with_config(|c| {
        c.trust_proxy_headers = true;
        c.login_limit.max_per_ip = 3;
    })
    .await;
    app.user("ae@agency.test", PASSWORD, false).await;
    for i in 0..3 {
        let r = app
            .call(
                Req::post("/v1/auth/login")
                    .header("cf-connecting-ip", "203.0.113.7")
                    .json(json!({ "email": format!("u{i}@agency.test"), "password": "x" })),
            )
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    let r = app
        .call(
            Req::post("/v1/auth/login")
                .header("cf-connecting-ip", "203.0.113.7")
                .json(json!({ "email": "ae@agency.test", "password": PASSWORD })),
        )
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    // 다른 IP에서는 정상 로그인
    let r = app
        .call(
            Req::post("/v1/auth/login")
                .header("cf-connecting-ip", "203.0.113.8")
                .json(json!({ "email": "ae@agency.test", "password": PASSWORD })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_rotates_session_against_fixation() {
    let app = TestApp::new().await;
    app.user("ae@agency.test", PASSWORD, false).await;
    let first = app.login("ae@agency.test", PASSWORD).await;

    // 이미 쿠키를 가진 채로 다시 로그인하면 새 토큰을 받고 옛 토큰은 무효가 된다.
    let r = app
        .call(
            Req::post("/v1/auth/login")
                .cookie(&first)
                .json(json!({ "email": "ae@agency.test", "password": PASSWORD })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let second = r.session_cookie().unwrap();
    assert_ne!(first, second);
    let old = app.call(Req::get("/v1/auth/me").cookie(&first)).await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);
    let new = app.call(Req::get("/v1/auth/me").cookie(&second)).await;
    assert_eq!(new.status, StatusCode::OK);

    // 공격자가 심은 임의 토큰은 로그인 후에도 세션이 되지 않는다.
    let planted = format!("at_session={}", "ab".repeat(32));
    let r = app
        .call(
            Req::post("/v1/auth/login")
                .cookie(&planted)
                .json(json!({ "email": "ae@agency.test", "password": PASSWORD })),
        )
        .await;
    assert_ne!(r.session_cookie().unwrap(), planted);
    let me = app.call(Req::get("/v1/auth/me").cookie(&planted)).await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn forged_or_tampered_cookies_are_rejected() {
    let app = TestApp::new().await;
    let (_, cookie) = app.member("ae@agency.test").await;
    let value = cookie.trim_start_matches("at_session=").to_owned();

    // 한 글자 바꾼 토큰
    let mut chars: Vec<char> = value.chars().collect();
    chars[10] = if chars[10] == 'a' { 'b' } else { 'a' };
    let tampered: String = chars.into_iter().collect();

    // DB에 저장된 값(sha256)을 그대로 쿠키로 보내도 통과하면 안 된다.
    let stored: String = sqlx::query_scalar("SELECT id FROM sessions")
        .fetch_one(&app.db.pg)
        .await
        .unwrap();

    for bad in [
        format!("at_session={tampered}"),
        format!("at_session={stored}"),
        "at_session=".to_owned(),
        "at_session=' OR 1=1 --".to_owned(),
        format!("at_session={}", "z".repeat(64)),
        format!("at_session={}", "a".repeat(10_000)),
        format!("other=1; at_session={}", value.to_uppercase()),
    ] {
        let r = app.call(Req::get("/v1/auth/me").cookie(&bad)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{bad}");
    }
    // 다른 쿠키와 섞여 있어도 올바른 토큰은 통과
    let ok = app
        .call(Req::get("/v1/auth/me").cookie(&format!("foo=bar; {cookie}; x=y")))
        .await;
    assert_eq!(ok.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn password_hash_never_in_responses() {
    let app = TestApp::new().await;
    let root = app.super_admin().await;
    let r = app
        .call(Req::post("/v1/users").cookie(&root).json(json!({
            "email": "new@agency.test", "password": "initial password 1", "name": "신규"
        })))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let login = app
        .login_resp("new@agency.test", "initial password 1")
        .await;
    let me = app.call(Req::get("/v1/auth/me").cookie(&root)).await;
    let users = app.call(Req::get("/v1/users").cookie(&root)).await;
    for resp in [&r, &login, &me, &users] {
        assert!(!resp.text.contains("argon2"), "{}", resp.text);
        assert!(!resp.text.contains("password"), "{}", resp.text);
    }
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE payload::text ILIKE '%password%' OR payload::text ILIKE '%argon2%'",
    )
    .fetch_one(&app.db.pg)
    .await
    .unwrap();
    assert_eq!(leaked, 0, "감사 로그에도 비밀번호를 남기지 않는다");
}

//! 로그인.

use crate::Route;
use crate::api::{self, LOGIN_PATH, Method};
use crate::components::ui::{Button, Size, TextField};
use crate::logic::validate;
use crate::state::use_app;
use analytics_core::dto::admin::{LoginRequest, MeResponse};
use dioxus::prelude::*;

/// `next`는 같은 사이트 안의 경로만 허용한다 (오픈 리다이렉트 방지).
pub fn safe_next(next: &str) -> Option<String> {
    // 역슬래시·제어 문자는 어디에 있든 거부한다 (브라우저가 `\`를 `/`로 바꾸거나 탭·줄바꿈을 지워
    // `//evil.com`이 되는 경우를 막는다).
    let ok = next.starts_with('/')
        && !next.starts_with("//")
        && !next.starts_with("/login")
        && !next.chars().any(|c| c == '\\' || c.is_control());
    ok.then(|| next.to_owned())
}

/// 로그인 응답은 받았는데 쿠키가 저장되지 않은 경우의 안내.
pub const COOKIE_BLOCKED: &str = "로그인은 됐지만 브라우저가 로그인 쿠키를 막고 있어요. 쿠키 차단(시크릿 모드·추적 방지)을 풀거나 다른 브라우저로 다시 시도해 주세요";

/// 로그인 직후 `/v1/auth/me` 확인 결과 → 계속 진행할지, 보여줄 오류.
/// 401이면 쿠키가 저장되지 않은 것이다. 그 밖의 오류(네트워크 등)는 판단할 수 없으니 진행한다.
pub fn session_check(r: &crate::api::ApiResult<MeResponse>) -> Result<(), &'static str> {
    match r {
        Err(crate::logic::errors::ApiError::Unauthorized) => Err(COOKIE_BLOCKED),
        _ => Ok(()),
    }
}

/// 데모 빌드: 미리 채울 (이메일, 비밀번호). 데모 서버는 어떤 값으로도 로그인시킨다.
#[cfg(feature = "demo")]
const DEMO: Option<(&str, &str)> = Some((crate::demo::DEMO_EMAIL, "demo1234"));
#[cfg(not(feature = "demo"))]
const DEMO: Option<(&str, &str)> = None;

/// 데모 로그인 화면 안내
pub const DEMO_HINT: &str = "데모입니다. 아무 이메일·비밀번호로 로그인할 수 있어요.";

#[component]
pub fn Login(next: String) -> Element {
    let mut ctx = use_app();
    let mut email = use_signal(|| DEMO.map(|d| d.0.to_string()).unwrap_or_default());
    let mut password = use_signal(|| DEMO.map(|d| d.1.to_string()).unwrap_or_default());
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut touched = use_signal(|| false);

    let email_err = (DEMO.is_none() && touched() && validate::email(&email()).is_err())
        .then(|| "올바른 이메일 주소를 입력하세요".to_string());

    let mut submit = move || {
        touched.set(true);
        // 데모는 입력 검사 없이 그대로 보낸다 (빈 값도 들어간다)
        let e = match (DEMO, validate::email(&email())) {
            (Some(_), _) => email(),
            (None, Ok(e)) => e,
            (None, Err(_)) => return,
        };
        if DEMO.is_none() && password().is_empty() {
            error.set(Some("비밀번호를 입력하세요".into()));
            return;
        }
        busy.set(true);
        error.set(None);
        let req = LoginRequest {
            email: e,
            password: password(),
        };
        let next = next.clone();
        spawn(async move {
            match api::send::<_, MeResponse>(Method::Post, LOGIN_PATH, &req).await {
                Ok(me) => {
                    // 로그인 응답만으로는 쿠키가 저장됐는지 알 수 없다. 쿠키로 한 번 더 확인한다.
                    let check = api::get::<MeResponse>("/v1/auth/me").await;
                    if let Err(msg) = session_check(&check) {
                        error.set(Some(msg.into()));
                        busy.set(false);
                        return;
                    }
                    ctx.me.set(Some(check.unwrap_or(me)));
                    match safe_next(&next) {
                        Some(p) => {
                            navigator().replace(p);
                        }
                        None => {
                            navigator().replace(Route::Home {});
                        }
                    }
                }
                Err(e) => error.set(Some(e.message())),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "login-page",
            section { class: "login-hero",
                crate::components::brand::LoginWordmark {}
                div {
                    h2 { "광고 성과를" br {} "실제 매출로 봅니다" }
                    p { "매체마다 따로 잡히던 매출을 주문번호 기준으로 한 번만 세고, 기여 모델별로 나눠 보여줍니다." }
                }
                span { class: "login-tag", "BinaryCube Analytics" }
            }
            div { class: "login-panel",
            form {
                class: "card login-card",
                // 데모: 브라우저의 이메일 형식 검사도 끈다 (아무 값으로 들어간다)
                novalidate: DEMO.is_some(),
                onsubmit: move |e| {
                    e.prevent_default();
                    submit();
                },
                crate::components::brand::BrandSymbol { size: 40 }
                h1 { class: "login-title", "로그인" }
                p { class: "page-desc", "광고 성과와 실제 매출을 한 곳에서 확인하세요." }
                if DEMO.is_some() {
                    p { class: "demo-hint", role: "note", "{DEMO_HINT}" }
                }
                div { class: "login-fields",
                    TextField {
                        label: "이메일",
                        kind: "email",
                        autocomplete: "username",
                        value: email(),
                        placeholder: "name@company.com",
                        oninput: move |v| email.set(v),
                        error: email_err,
                    }
                    TextField {
                        label: "비밀번호",
                        kind: "password",
                        autocomplete: "current-password",
                        value: password(),
                        oninput: move |v| password.set(v),
                    }
                }
                if let Some(e) = error() {
                    div { class: "form-error", role: "alert", "{e}" }
                }
                Button { size: Size::Lg, disabled: busy(), if busy() { "확인 중…" } else { "로그인" } }
                if DEMO.is_none() {
                    p { class: "login-foot", "계정이 없으면 대행사 담당자에게 요청하세요." }
                }
            }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::safe_next;

    #[test]
    fn next_must_be_local_path() {
        assert_eq!(
            safe_next("/p/1/pages?x=1").as_deref(),
            Some("/p/1/pages?x=1")
        );
        assert_eq!(safe_next("https://evil.com"), None);
        assert_eq!(safe_next("//evil.com"), None);
        assert_eq!(safe_next("/\\evil.com"), None);
        assert_eq!(safe_next("/login?next=/x"), None);
        assert_eq!(safe_next(""), None);
        assert_eq!(safe_next("javascript:alert(1)"), None);
    }

    #[test]
    fn next_rejects_backslash_and_control_chars_anywhere() {
        assert_eq!(safe_next("/p/1\\..\\evil"), None);
        assert_eq!(safe_next("/a\\b"), None);
        assert_eq!(safe_next("/p/1\n/x"), None);
        assert_eq!(safe_next("/\t/evil.com"), None);
        assert_eq!(safe_next("/p\u{7f}"), None);
        assert_eq!(safe_next("/p/1/설정").as_deref(), Some("/p/1/설정"));
    }

    #[test]
    fn me_401_after_login_means_cookie_blocked() {
        use super::{COOKIE_BLOCKED, session_check};
        use crate::logic::errors::ApiError;
        assert_eq!(
            session_check(&Err(ApiError::Unauthorized)),
            Err(COOKIE_BLOCKED)
        );
        assert_eq!(session_check(&Err(ApiError::Network)), Ok(()));
        assert!(COOKIE_BLOCKED.contains("브라우저가 로그인 쿠키를 막고 있어요"));
    }
}

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
    let ok = next.starts_with('/')
        && !next.starts_with("//")
        && !next.starts_with("/\\")
        && !next.starts_with("/login");
    ok.then(|| next.to_owned())
}

#[component]
pub fn Login(next: String) -> Element {
    let mut ctx = use_app();
    let mut email = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut touched = use_signal(|| false);

    let email_err = (touched() && validate::email(&email()).is_err())
        .then(|| "올바른 이메일 주소를 입력하세요".to_string());

    let mut submit = move || {
        touched.set(true);
        let Ok(e) = validate::email(&email()) else {
            return;
        };
        if password().is_empty() {
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
                    ctx.me.set(Some(me));
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
        div { class: "center-page login-page",
            form {
                class: "card login-card",
                onsubmit: move |e| {
                    e.prevent_default();
                    submit();
                },
                div { class: "brand login-brand",
                    div { class: "brand-mark", "22" }
                    "Analytics"
                }
                h1 { class: "login-title", "로그인" }
                p { class: "page-desc", "광고 성과와 실제 매출을 한 곳에서 확인하세요." }
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
                p { class: "login-foot", "계정이 없으면 대행사 담당자에게 요청하세요." }
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
}

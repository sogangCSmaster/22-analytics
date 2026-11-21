//! 브라우저 기능 (시계, localStorage, 타이머, 이동). 네이티브 빌드(테스트)에서는 빈 구현.

#[cfg(target_arch = "wasm32")]
mod imp {
    pub fn now_ms() -> i64 {
        js_sys::Date::now() as i64
    }

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    pub fn storage_get(key: &str) -> Option<String> {
        storage()?.get_item(key).ok().flatten()
    }

    pub fn storage_set(key: &str, value: &str) {
        if let Some(s) = storage() {
            let _ = s.set_item(key, value);
        }
    }

    pub async fn sleep_ms(ms: u32) {
        gloo_timers::future::TimeoutFuture::new(ms).await;
    }

    /// 탭이 숨겨져 있는지 (다른 탭·최소화). 알 수 없으면 보이는 것으로 본다.
    pub fn document_hidden() -> bool {
        web_sys::window()
            .and_then(|w| w.document())
            .is_some_and(|d| d.hidden())
    }

    /// 현재 경로 + 쿼리 (`/p/1/pages?x=1`)
    pub fn current_path() -> String {
        web_sys::window()
            .and_then(|w| {
                let l = w.location();
                Some(format!("{}{}", l.pathname().ok()?, l.search().ok()?))
            })
            .unwrap_or_else(|| "/".into())
    }

    /// 세션 만료: 로그인 화면으로 (전체 새로 고침으로 상태를 비운다).
    pub fn redirect_to_login() {
        let next = current_path();
        if next.starts_with("/login") {
            return;
        }
        if let Some(w) = web_sys::window() {
            let url = format!("/login?next={}", crate::logic::query::encode(&next));
            let _ = w.location().replace(&url);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    pub fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    pub fn storage_get(_key: &str) -> Option<String> {
        None
    }

    pub fn storage_set(_key: &str, _value: &str) {}

    pub async fn sleep_ms(_ms: u32) {}

    #[allow(dead_code)]
    pub fn document_hidden() -> bool {
        false
    }

    #[allow(dead_code)]
    pub fn current_path() -> String {
        "/".into()
    }

    pub fn redirect_to_login() {}
}

pub use imp::*;

/// 텍스트 파일 내려받기 (CSV 등). 파일 내용은 JSON 문자열로 넘겨 스크립트 주입을 막는다.
pub fn download_text(file_name: &str, mime: &str, text: &str) {
    let js = format!(
        "const b = new Blob([\"\\ufeff\" + {}], {{ type: {} }});
         const a = document.createElement('a');
         a.href = URL.createObjectURL(b); a.download = {};
         document.body.appendChild(a); a.click(); a.remove();
         setTimeout(() => URL.revokeObjectURL(a.href), 1000);",
        crate::components::ui::js_string(text),
        crate::components::ui::js_string(mime),
        crate::components::ui::js_string(file_name),
    );
    let _ = dioxus::prelude::document::eval(&js);
}

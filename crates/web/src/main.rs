//! 22-analytics 대시보드 (Dioxus Web SPA).
//!
//! - API: [`api`] (fetch, 쿠키 세션), 경로·쿼리는 [`logic::query`]
//! - 상태: [`state::AppCtx`] (로그인 사용자, 기간·비교·선택 프로젝트는 localStorage에 저장)
//! - 순수 로직은 [`logic`]에 두고 `cargo test -p analytics-web --no-default-features`로 검증한다.

mod api;
mod components;
#[cfg(any(feature = "demo", test))]
mod demo;
mod format;
mod logic;
mod pages;
mod platform;
mod state;

use dioxus::prelude::*;

use components::shell::{AuthGate, Shell};
use pages::{
    acquisition::Acquisition, admin::Admin, events::Events, funnel::Funnel, gallery::Gallery,
    home::Home, login::Login, not_found::NotFound, overview::Overview, page_report::Pages,
    realtime::Realtime, revenue::Revenue, settings::Settings,
};

const TOKENS_CSS: Asset = asset!("/assets/tokens.css");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const APP_CSS: Asset = asset!("/assets/app.css");
const PRETENDARD: &str = "https://cdn.jsdelivr.net/gh/orioncactus/pretendard@v1.3.9/dist/web/variable/pretendardvariable-dynamic-subset.min.css";

#[derive(Routable, Clone, PartialEq, Debug)]
#[rustfmt::skip]
#[allow(clippy::enum_variant_names)]
pub enum Route {
    #[route("/login?:next")]
    Login { next: String },
    #[layout(AuthGate)]
        #[route("/")]
        Home {},
        #[layout(Shell)]
            #[nest("/p/:pid")]
                #[route("/")]
                Overview { pid: i32 },
                #[route("/realtime")]
                Realtime { pid: i32 },
                #[route("/acquisition")]
                Acquisition { pid: i32 },
                #[route("/pages")]
                Pages { pid: i32 },
                #[route("/events")]
                Events { pid: i32 },
                #[route("/funnel")]
                Funnel { pid: i32 },
                #[route("/revenue")]
                Revenue { pid: i32 },
                #[route("/settings?:tab")]
                Settings { pid: i32, tab: String },
            #[end_nest]
            #[route("/admin")]
            Admin {},
            #[route("/dev/components")]
            Gallery {},
        #[end_layout]
    #[end_layout]
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

impl Route {
    /// 현재 화면의 프로젝트 id (프로젝트 화면이 아니면 None)
    pub fn pid(&self) -> Option<i32> {
        match self {
            Route::Overview { pid }
            | Route::Realtime { pid }
            | Route::Acquisition { pid }
            | Route::Pages { pid }
            | Route::Events { pid }
            | Route::Funnel { pid }
            | Route::Revenue { pid }
            | Route::Settings { pid, .. } => Some(*pid),
            _ => None,
        }
    }

    /// 같은 화면을 다른 프로젝트로 (프로젝트 전환기)
    pub fn with_pid(&self, pid: i32) -> Route {
        match self {
            Route::Realtime { .. } => Route::Realtime { pid },
            Route::Acquisition { .. } => Route::Acquisition { pid },
            Route::Pages { .. } => Route::Pages { pid },
            Route::Events { .. } => Route::Events { pid },
            Route::Funnel { .. } => Route::Funnel { pid },
            Route::Revenue { .. } => Route::Revenue { pid },
            Route::Settings { tab, .. } => Route::Settings {
                pid,
                tab: tab.clone(),
            },
            _ => Route::Overview { pid },
        }
    }

    /// 기간 선택이 의미 있는 화면인지
    pub fn uses_range(&self) -> bool {
        matches!(
            self,
            Route::Overview { .. }
                | Route::Acquisition { .. }
                | Route::Pages { .. }
                | Route::Events { .. }
                | Route::Funnel { .. }
                | Route::Revenue { .. }
        )
    }
}

fn main() {
    dioxus::launch(App);
}

#[cfg(test)]
mod route_tests {
    use super::Route;

    /// 기간 선택·이전 기간 비교 토글은 기간이 의미 있는 화면에만 보인다.
    #[test]
    fn range_and_compare_only_on_report_pages() {
        for r in [
            Route::Overview { pid: 1 },
            Route::Acquisition { pid: 1 },
            Route::Pages { pid: 1 },
            Route::Events { pid: 1 },
            Route::Funnel { pid: 1 },
            Route::Revenue { pid: 1 },
        ] {
            assert!(r.uses_range(), "{r:?}");
        }
        for r in [
            Route::Realtime { pid: 1 },
            Route::Settings {
                pid: 1,
                tab: String::new(),
            },
            Route::Admin {},
            Route::Gallery {},
            Route::Home {},
        ] {
            assert!(!r.uses_range(), "{r:?}");
        }
    }
}

#[component]
fn App() -> Element {
    state::AppCtx::provide();
    rsx! {
        document::Title { "BinaryCube Analytics" }
        document::Link { rel: "stylesheet", href: PRETENDARD }
        document::Link { rel: "icon", r#type: "image/png", href: components::brand::FAVICON }
        document::Stylesheet { href: TOKENS_CSS }
        document::Stylesheet { href: MAIN_CSS }
        document::Stylesheet { href: APP_CSS }
        Router::<Route> {}
        components::ui::ToastHost {}
    }
}

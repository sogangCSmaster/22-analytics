//! 22-analytics 대시보드 (Dioxus Web SPA).
//!
//! Phase 0: 디자인 토큰, 기본 컴포넌트, 예시 데이터로 만든 화면.
//! Phase 1부터 api 서버와 연결한다.

mod components;
mod format;
mod logic;
mod mock;
mod pages;

use dioxus::prelude::*;

use components::shell::Shell;
use pages::{
    gallery::Gallery, install::Install, not_found::NotFound, overview::Overview, revenue::Revenue,
};

const TOKENS_CSS: Asset = asset!("/assets/tokens.css");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const PRETENDARD: &str = "https://cdn.jsdelivr.net/gh/orioncactus/pretendard@v1.3.9/dist/web/variable/pretendardvariable-dynamic-subset.min.css";

#[derive(Routable, Clone, PartialEq, Debug)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Overview {},
        #[route("/revenue")]
        Revenue {},
        #[route("/install")]
        Install {},
        #[route("/dev/components")]
        Gallery {},
    #[end_layout]
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Title { "22 Analytics" }
        document::Link { rel: "stylesheet", href: PRETENDARD }
        document::Stylesheet { href: TOKENS_CSS }
        document::Stylesheet { href: MAIN_CSS }
        Router::<Route> {}
    }
}

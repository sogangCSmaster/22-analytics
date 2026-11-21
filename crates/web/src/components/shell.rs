//! 공통 레이아웃: 왼쪽 사이드바 + 상단 바 + 본문.

use crate::Route;
use crate::components::ui::SegmentedControl;
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Period {
    Today,
    Days7,
    Days30,
}

#[component]
pub fn Shell() -> Element {
    let mut period = use_context_provider(|| Signal::new(Period::Days7));
    rsx! {
        div { class: "shell",
            aside { class: "sidebar",
                div { class: "brand",
                    div { class: "brand-mark", "22" }
                    "Analytics"
                }
                div { class: "project-switch", title: "광고주·사이트 전환 (Phase 1)",
                    span { class: "org", "개발용 광고주" }
                    span { class: "name", "로컬 테스트 사이트" }
                }
                nav { class: "nav",
                    div { class: "nav-section", "리포트" }
                    NavLink { to: Route::Overview {}, label: "개요" }
                    NavLink { to: Route::Revenue {}, label: "실제 매출" }
                    div { class: "nav-section", "설정" }
                    NavLink { to: Route::Install {}, label: "설치·수집 설정" }
                    div { class: "nav-section", "개발" }
                    NavLink { to: Route::Gallery {}, label: "컴포넌트" }
                }
            }
            div { class: "main",
                header { class: "topbar",
                    SegmentedControl {
                        options: vec![
                            (Period::Today, "오늘".to_string()),
                            (Period::Days7, "최근 7일".to_string()),
                            (Period::Days30, "최근 30일".to_string()),
                        ],
                        value: period(),
                        onchange: move |p| period.set(p),
                    }
                    div { class: "topbar-right",
                        span { class: "badge badge-warn", "예시 데이터" }
                        div { class: "avatar", "관" }
                    }
                }
                main { class: "content", Outlet::<Route> {} }
            }
        }
    }
}

#[component]
fn NavLink(to: Route, label: String) -> Element {
    rsx! {
        Link { to, active_class: "active",
            span { class: "nav-dot" }
            "{label}"
        }
    }
}

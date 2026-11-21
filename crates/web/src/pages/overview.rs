use crate::components::chart::{BarList, TrendChart};
use crate::components::ui::{Card, StatCard};
use crate::{format, mock};
use dioxus::prelude::*;

#[component]
pub fn Overview() -> Element {
    let daily = mock::daily_visitors();
    let values: Vec<i64> = daily.iter().map(|d| d.visitors).collect();
    let labels: Vec<String> = daily.iter().map(|d| d.label.to_string()).collect();
    let channels: Vec<(String, i64)> = mock::top_channels()
        .into_iter()
        .map(|(l, v)| (l.to_string(), v))
        .collect();

    rsx! {
        div { class: "page-head",
            div {
                h1 { class: "page-title", "개요" }
                p { class: "page-desc", "방문과 매출을 한눈에 봅니다." }
            }
        }
        div { class: "grid grid-4",
            StatCard { label: "방문자", value: format::int(48_210), delta: 0.082, sub: "지난 기간 대비" }
            StatCard { label: "세션", value: format::int(71_930), delta: 0.054, sub: "지난 기간 대비" }
            StatCard { label: "페이지뷰", value: format::int(203_118), delta: -0.021, sub: "지난 기간 대비" }
            StatCard { label: "실제 매출", value: format::won_short(130_000_000), delta: 0.113, sub: "주문번호 기준" }
        }
        Card { title: "일별 방문자",
            TrendChart { values, labels }
        }
        div { class: "grid grid-2",
            Card { title: "유입 채널 (세션)",
                BarList { items: channels }
            }
            Card { title: "많이 본 페이지",
                div { class: "table-wrap",
                    table { class: "table",
                        thead {
                            tr {
                                th { "페이지" }
                                th { class: "r", "페이지뷰" }
                                th { class: "r", "방문자" }
                                th { class: "r", "이탈률" }
                            }
                        }
                        tbody {
                            for p in mock::top_pages() {
                                tr { key: "{p.path}",
                                    td { "{p.path}" }
                                    td { class: "r", {format::int(p.pageviews)} }
                                    td { class: "r", {format::int(p.visitors)} }
                                    td { class: "r muted", {format::pct(p.bounce, 0)} }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

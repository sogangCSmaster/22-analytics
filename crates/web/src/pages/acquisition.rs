//! 유입(채널): 어떤 채널이 세션을 데려왔는지. 막대 + 정렬 가능한 표.

use crate::api;
use crate::components::chart::{BarList, ShareBar};
use crate::components::ui::{Card, CsvButton, EmptyState, PageHead, SkeletonBlock, SortTh, load};
use crate::format;
use crate::logic::table::{Sort, cmp_text, sort_rows};
use crate::logic::{labels, query, query::Report};
use crate::pages::overview::{per, top_with_rest};
use crate::state::use_app;
use analytics_core::dto::reports::ChannelTrafficRow;
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Col {
    Channel,
    Sessions,
    Visitors,
    Pageviews,
    PerSession,
}

#[component]
pub fn Acquisition(pid: ReadSignal<i32>) -> Element {
    let ctx = use_app();
    let sort = use_signal(|| Sort::desc(Col::Sessions));
    let res = use_resource(move || {
        let (p, r) = (pid(), ctx.range());
        async move { api::get::<Vec<ChannelTrafficRow>>(&query::report(p, Report::Channels, r)).await }
    });
    let range = ctx.range();

    rsx! {
        PageHead { title: "유입", desc: format!("{} · 세션을 시작한 채널 기준", range.label()) }
        {load(res, rsx! { Card { SkeletonBlock { rows: 8 } } }, |mut rows| {
            if rows.is_empty() {
                return rsx! {
                    Card { EmptyState { icon: "↗", title: "이 기간에 유입이 없어요", desc: "기간을 넓히거나 설치 상태를 확인해 주세요." } }
                };
            }
            let total_sessions: u64 = rows.iter().map(|r| r.sessions).sum();
            sort_rows(&mut rows, sort(), |a, b, k| match k {
                Col::Channel => cmp_text(&labels::channel(&a.channel), &labels::channel(&b.channel)),
                Col::Sessions => a.sessions.cmp(&b.sessions),
                Col::Visitors => a.visitors.cmp(&b.visitors),
                Col::Pageviews => a.pageviews.cmp(&b.pageviews),
                Col::PerSession => (a.pageviews * b.sessions.max(1)).cmp(&(b.pageviews * a.sessions.max(1))),
            });
            let items: Vec<(String, i64)> = rows.iter().map(|r| (labels::channel(&r.channel), r.sessions as i64)).collect();
            let csv_rows: Vec<Vec<String>> = rows.iter().map(|r| vec![labels::channel(&r.channel), r.channel.clone(), r.sessions.to_string(), r.visitors.to_string(), r.pageviews.to_string()]).collect();
            rsx! {
                div { class: "grid grid-2",
                    Card { title: "채널 구성",
                        ShareBar { parts: top_with_rest(&items, 4) }
                        div { class: "callout", style: "margin-top: 20px",
                            strong { "{items.iter().max_by_key(|i| i.1).map(|i| i.0.clone()).unwrap_or_default()}" }
                            "이(가) 세션을 가장 많이 데려왔어요. 전체 세션 {format::count(total_sessions)}개."
                        }
                    }
                    Card { title: "채널별 세션",
                        BarList { items: top_with_rest(&items, 7), show_share: true }
                    }
                }
                Card {
                    title: "채널 상세",
                    actions: rsx! { CsvButton { file_name: format!("유입_{}.csv", range.label()), headers: vec!["채널".into(), "채널 ID".into(), "세션".into(), "방문자".into(), "페이지뷰".into()], rows: csv_rows } },
                    div { class: "table-wrap",
                        table { class: "table",
                            thead {
                                tr {
                                    SortTh { label: "채널", k: Col::Channel, sort, right: false }
                                    SortTh { label: "세션", k: Col::Sessions, sort }
                                    th { class: "r", scope: "col", "비중" }
                                    SortTh { label: "방문자", k: Col::Visitors, sort }
                                    SortTh { label: "페이지뷰", k: Col::Pageviews, sort }
                                    SortTh { label: "세션당 페이지뷰", k: Col::PerSession, sort }
                                }
                            }
                            tbody {
                                for r in rows.iter() {
                                    tr { key: "{r.channel}",
                                        td {
                                            div { class: "cell-title", "{labels::channel(&r.channel)}" }
                                            if labels::channel(&r.channel) != r.channel {
                                                div { class: "cell-sub", "{r.channel}" }
                                            }
                                        }
                                        td { class: "r", "{format::count(r.sessions)}" }
                                        td { class: "r muted", {format::pct(if total_sessions > 0 { r.sessions as f64 / total_sessions as f64 } else { 0.0 }, 1)} }
                                        td { class: "r", "{format::count(r.visitors)}" }
                                        td { class: "r", "{format::count(r.pageviews)}" }
                                        td { class: "r", "{per(r.pageviews, r.sessions)}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })}
    }
}

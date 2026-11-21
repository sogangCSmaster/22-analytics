//! 페이지: 경로별 페이지뷰·방문자·시작 세션. 경로 검색(서버 검색, 입력 멈춤 0.3초 후).

use crate::api;
use crate::components::ui::{Card, CsvButton, EmptyState, PageHead, SkeletonBlock, SortTh, load};
use crate::format;
use crate::logic::query;
use crate::logic::table::{Sort, cmp_text, sort_rows};
use crate::platform::sleep_ms;
use crate::state::use_app;
use analytics_core::dto::reports::PageRow;
use dioxus::prelude::*;

pub const LIMIT: u32 = 200;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Col {
    Path,
    Pageviews,
    Visitors,
    Entrances,
}

#[component]
pub fn Pages(pid: ReadSignal<i32>) -> Element {
    let ctx = use_app();
    let mut input = use_signal(String::new);
    let sort = use_signal(|| Sort::desc(Col::Pageviews));
    let res = use_resource(move || {
        let (p, r, q) = (pid(), ctx.range(), input());
        async move {
            if !q.trim().is_empty() {
                // 타이핑 중 요청 줄이기: 새 입력이 오면 이 future는 취소된다.
                sleep_ms(300).await;
            }
            api::get::<Vec<PageRow>>(&query::pages(p, r, &q, LIMIT)).await
        }
    });
    let range = ctx.range();

    rsx! {
        PageHead { title: "페이지", desc: format!("{} · 많이 본 순 최대 {LIMIT}개", range.label()) }
        Card {
            div { class: "toolbar",
                input {
                    class: "input search",
                    r#type: "search",
                    aria_label: "경로 검색",
                    placeholder: "경로 검색 (예: /product)",
                    value: "{input}",
                    oninput: move |e| input.set(e.value()),
                }
                {
                    let rows: Vec<Vec<String>> = match &*res.read() {
                        Some(Ok(v)) => v.iter().map(|r| vec![r.path.clone(), r.pageviews.to_string(), r.visitors.to_string(), r.entrances.to_string()]).collect(),
                        _ => vec![],
                    };
                    rsx! { CsvButton { file_name: format!("페이지_{}.csv", range.label()), headers: vec!["경로".into(), "페이지뷰".into(), "방문자".into(), "시작 세션".into()], rows } }
                }
            }
            {load(res, rsx! { SkeletonBlock { rows: 10 } }, |mut rows| {
                if rows.is_empty() {
                    return rsx! {
                        EmptyState {
                            icon: "▤",
                            title: if input().trim().is_empty() { "페이지 조회가 없어요" } else { "검색 결과가 없어요" },
                            desc: if input().trim().is_empty() { "이 기간에 기록된 페이지뷰가 없습니다." } else { "경로 일부만 입력해 보세요. 대소문자는 구분하지 않아요." },
                        }
                    };
                }
                let total: u64 = rows.iter().map(|r| r.pageviews).sum();
                sort_rows(&mut rows, sort(), |a, b, k| match k {
                    Col::Path => cmp_text(&a.path, &b.path),
                    Col::Pageviews => a.pageviews.cmp(&b.pageviews),
                    Col::Visitors => a.visitors.cmp(&b.visitors),
                    Col::Entrances => a.entrances.cmp(&b.entrances),
                });
                rsx! {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead {
                                tr {
                                    SortTh { label: "페이지 경로", k: Col::Path, sort, right: false }
                                    SortTh { label: "페이지뷰", k: Col::Pageviews, sort }
                                    th { class: "r", scope: "col", "비중" }
                                    SortTh { label: "방문자", k: Col::Visitors, sort }
                                    SortTh { label: "시작 세션", k: Col::Entrances, sort }
                                }
                            }
                            tbody {
                                for (i, r) in rows.iter().enumerate() {
                                    tr { key: "{i}",
                                        td { class: "path-cell", title: "{r.path}", "{r.path}" }
                                        td { class: "r", "{format::count(r.pageviews)}" }
                                        td { class: "r muted", {format::pct(if total > 0 { r.pageviews as f64 / total as f64 } else { 0.0 }, 1)} }
                                        td { class: "r", "{format::count(r.visitors)}" }
                                        td { class: "r", "{format::count(r.entrances)}" }
                                    }
                                }
                            }
                        }
                    }
                }
            })}
        }
    }
}

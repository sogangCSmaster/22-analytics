//! 페이지: 경로별 페이지뷰·방문자·시작 세션. 경로 검색(서버 검색, 입력 멈춤 0.3초 후).
//! 이전 기간 비교를 켜면 페이지뷰의 이전 값과 변화가 붙는다.

use crate::api;
use crate::components::ui::{
    Card, ChangeTag, CmpCells, CsvButton, EmptyState, PageHead, SkeletonBlock, SortTh, load,
};
use crate::format;
use crate::logic::compare::{Change, change, cmp_change, is_complete, pair};
use crate::logic::query;
use crate::logic::table::{Sort, cmp_text, sort_rows};
use crate::platform::sleep_ms;
use crate::state::{loaded_prev, use_app};
use analytics_core::dto::reports::PageRow;
use dioxus::prelude::*;

pub const LIMIT: u32 = 200;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Col {
    Path,
    Pageviews,
    PageviewsChange,
    Visitors,
    Entrances,
}

/// 현재·이전 페이지를 경로로 맞춘다. 목록은 많이 본 순 최대 `limit`개라, 잘린 쪽은 신규·사라짐을 단정하지 않는다.
pub fn pair_pages(
    cur: &[PageRow],
    prev: Option<&[PageRow]>,
    limit: u32,
) -> Vec<(PageRow, Option<PageRow>)> {
    match prev {
        Some(p) => pair(
            cur,
            p,
            |r| r.path.clone(),
            |r| PageRow {
                path: r.path.clone(),
                pageviews: 0,
                visitors: 0,
                entrances: 0,
            },
            // 표에는 현재 기간 페이지만 보인다 (사라진 페이지는 붙이지 않는다)
            false,
            is_complete(p.len(), limit),
        ),
        None => cur.iter().map(|r| (r.clone(), None)).collect(),
    }
}

fn pv_change(r: &PageRow, p: &Option<PageRow>) -> Change {
    change(r.pageviews as f64, p.as_ref().map(|p| p.pageviews as f64))
}

#[component]
pub fn PagesTable(
    rows: Vec<(PageRow, Option<PageRow>)>,
    compare: bool,
    sort: Signal<Sort<Col>>,
) -> Element {
    let mut rows = rows;
    let total: u64 = rows.iter().map(|r| r.0.pageviews).sum();
    sort_rows(&mut rows, sort(), |a, b, k| match k {
        Col::Path => cmp_text(&format::path(&a.0.path), &format::path(&b.0.path)),
        Col::Pageviews => a.0.pageviews.cmp(&b.0.pageviews),
        Col::PageviewsChange => cmp_change(pv_change(&a.0, &a.1), pv_change(&b.0, &b.1)),
        Col::Visitors => a.0.visitors.cmp(&b.0.visitors),
        Col::Entrances => a.0.entrances.cmp(&b.0.entrances),
    });
    rsx! {
        div { class: "table-wrap",
            table { class: "table table-cards",
                thead {
                    tr {
                        SortTh { label: "페이지 경로", k: Col::Path, sort, right: false }
                        SortTh { label: "페이지뷰", k: Col::Pageviews, sort }
                        if compare {
                            th { class: "r cmp-col", scope: "col", "이전 기간" }
                            SortTh { label: "변화", k: Col::PageviewsChange, sort, class: "cmp-col m-hide" }
                        }
                        th { class: "r", scope: "col", "비중" }
                        SortTh { label: "방문자", k: Col::Visitors, sort }
                        SortTh { label: "시작 세션", k: Col::Entrances, sort }
                    }
                }
                tbody {
                    for (i, (r, p)) in rows.iter().enumerate() {
                        tr { key: "{i}",
                            td { class: "path-cell", "data-label": "페이지 경로", title: "{format::path(&r.path)}", "{format::path(&r.path)}" }
                            td { class: "r main-metric", "data-label": "페이지뷰",
                                "{format::count(r.pageviews)}"
                                if compare {
                                    ChangeTag { change: pv_change(r, p), class: "m-only" }
                                }
                            }
                            if compare {
                                CmpCells { prev: p.as_ref().map(|p| format::count(p.pageviews)), change: pv_change(r, p) }
                            }
                            td { class: "r muted", "data-label": "비중", {format::pct(if total > 0 { r.pageviews as f64 / total as f64 } else { 0.0 }, 1)} }
                            td { class: "r", "data-label": "방문자", "{format::count(r.visitors)}" }
                            td { class: "r", "data-label": "시작 세션", "{format::count(r.entrances)}" }
                        }
                    }
                }
            }
        }
    }
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
    // 이전 기간 (비교를 켰을 때만, 같은 검색어로)
    let prev_res = use_resource(move || {
        let (p, r, q, cmp) = (pid(), ctx.range(), input(), ctx.compare());
        async move {
            if !cmp {
                return Ok(None);
            }
            if !q.trim().is_empty() {
                sleep_ms(300).await;
            }
            api::get::<Vec<PageRow>>(&query::pages(p, r.previous(), &q, LIMIT))
                .await
                .map(Some)
        }
    });
    let prev = loaded_prev(&prev_res);
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
                        Some(Ok(v)) => v.iter().map(|r| vec![format::path(&r.path), r.pageviews.to_string(), r.visitors.to_string(), r.entrances.to_string()]).collect(),
                        _ => vec![],
                    };
                    rsx! { CsvButton { file_name: format!("페이지_{}.csv", range.label()), headers: vec!["경로".into(), "페이지뷰".into(), "방문자".into(), "시작 세션".into()], rows } }
                }
            }
            {load(res, rsx! { SkeletonBlock { rows: 10 } }, |rows| {
                if rows.is_empty() {
                    return rsx! {
                        EmptyState {
                            icon: crate::components::icon::IconName::Pages,
                            title: if input().trim().is_empty() { "페이지 조회가 없어요" } else { "검색 결과가 없어요" },
                            desc: if input().trim().is_empty() { "이 기간에 기록된 페이지뷰가 없습니다." } else { "경로 일부만 입력해 보세요. 대소문자는 구분하지 않아요." },
                        }
                    };
                }
                let compare = prev.is_some();
                let paired = pair_pages(&rows, prev.as_deref(), LIMIT);
                rsx! { PagesTable { rows: paired, compare, sort } }
            })}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(p: &str, v: u64) -> PageRow {
        PageRow {
            path: p.into(),
            pageviews: v,
            visitors: v,
            entrances: 0,
        }
    }

    #[test]
    fn pages_pair_respects_truncated_lists() {
        // 이전 목록이 다 왔으면 없는 경로는 신규
        let p = pair_pages(
            &[r("/a", 3), r("/new", 1)],
            Some(&[r("/a", 6), r("/gone", 2)]),
            200,
        );
        assert_eq!(p.len(), 2, "사라진 페이지는 붙이지 않는다");
        assert_eq!(pv_change(&p[0].0, &p[0].1), Change::Pct(-0.5));
        assert_eq!(pv_change(&p[1].0, &p[1].1), Change::New);
        // 이전 목록이 한도까지 찼으면 없는 경로의 이전 값은 모른다
        let p = pair_pages(&[r("/new", 1)], Some(&[r("/a", 6)]), 1);
        assert_eq!(pv_change(&p[0].0, &p[0].1), Change::Unknown);
    }
}

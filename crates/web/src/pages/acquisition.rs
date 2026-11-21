//! 유입(채널): 어떤 채널이 세션을 데려왔는지. 막대 + 정렬 가능한 표.
//! 이전 기간 비교를 켜면 세션·방문자의 이전 값과 변화가 붙는다.

use crate::api;
use crate::components::chart::{BarList, ShareBar};
use crate::components::ui::{
    Card, ChangeTag, CmpCells, CmpHead, CsvButton, EmptyState, PageHead, SkeletonBlock, SortTh,
    load,
};
use crate::format;
use crate::logic::compare::{Change, change, cmp_change, pair, top_with_rest_prev};
use crate::logic::table::{Sort, cmp_text, sort_rows};
use crate::logic::{labels, query, query::Report};
use crate::pages::overview::{per, top_with_rest};
use crate::state::{loaded_prev, use_app, use_previous};
use analytics_core::dto::reports::ChannelTrafficRow;
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Col {
    Channel,
    Sessions,
    SessionsChange,
    Visitors,
    Pageviews,
    PerSession,
}

fn zero(r: &ChannelTrafficRow) -> ChannelTrafficRow {
    ChannelTrafficRow {
        channel: r.channel.clone(),
        sessions: 0,
        entrances: 0,
        visitors: 0,
        pageviews: 0,
    }
}

/// 채널 목록은 잘리지 않으므로 이전에 없던 채널은 신규, 사라진 채널은 0으로 붙인다.
pub fn pair_channels(
    cur: &[ChannelTrafficRow],
    prev: Option<&[ChannelTrafficRow]>,
) -> Vec<(ChannelTrafficRow, Option<ChannelTrafficRow>)> {
    match prev {
        Some(p) => pair(cur, p, |r| r.channel.clone(), zero, true, true),
        None => cur.iter().map(|r| (r.clone(), None)).collect(),
    }
}

fn sessions_change(r: &ChannelTrafficRow, p: &Option<ChannelTrafficRow>) -> Change {
    change(r.sessions as f64, p.as_ref().map(|p| p.sessions as f64))
}

/// 채널 상세 표 (비교를 켜면 이전 기간·변화 열이 붙는다).
#[component]
pub fn ChannelTable(
    rows: Vec<(ChannelTrafficRow, Option<ChannelTrafficRow>)>,
    compare: bool,
    sort: Signal<Sort<Col>>,
) -> Element {
    let mut rows = rows;
    let total_sessions: u64 = rows.iter().map(|r| r.0.sessions).sum();
    sort_rows(&mut rows, sort(), |a, b, k| match k {
        Col::Channel => cmp_text(
            &labels::channel(&a.0.channel),
            &labels::channel(&b.0.channel),
        ),
        Col::Sessions => a.0.sessions.cmp(&b.0.sessions),
        Col::SessionsChange => cmp_change(sessions_change(&a.0, &a.1), sessions_change(&b.0, &b.1)),
        Col::Visitors => a.0.visitors.cmp(&b.0.visitors),
        Col::Pageviews => a.0.pageviews.cmp(&b.0.pageviews),
        Col::PerSession => {
            (a.0.pageviews * b.0.sessions.max(1)).cmp(&(b.0.pageviews * a.0.sessions.max(1)))
        }
    });
    rsx! {
        div { class: "table-wrap",
            table { class: "table table-cards",
                thead {
                    tr {
                        SortTh { label: "채널", k: Col::Channel, sort, right: false }
                        SortTh { label: "세션", k: Col::Sessions, sort }
                        if compare {
                            th { class: "r cmp-col", scope: "col", "이전 기간" }
                            SortTh { label: "변화", k: Col::SessionsChange, sort, class: "cmp-col m-hide" }
                        }
                        th { class: "r", scope: "col", "비중" }
                        SortTh { label: "방문자", k: Col::Visitors, sort }
                        if compare {
                            CmpHead { change: "방문자 변화", change_only: true }
                        }
                        SortTh { label: "페이지뷰", k: Col::Pageviews, sort }
                        SortTh { label: "세션당 페이지뷰", k: Col::PerSession, sort }
                    }
                }
                tbody {
                    for (r, p) in rows.iter() {
                        tr { key: "{r.channel}",
                            td { "data-label": "채널",
                                div { class: "cell-title", "{labels::channel(&r.channel)}" }
                                if labels::channel(&r.channel) != r.channel {
                                    div { class: "cell-sub", "{r.channel}" }
                                }
                            }
                            td { class: "r main-metric", "data-label": "세션",
                                "{format::count(r.sessions)}"
                                if compare {
                                    ChangeTag { change: sessions_change(r, p), class: "m-only" }
                                }
                            }
                            if compare {
                                CmpCells { prev: p.as_ref().map(|p| format::count(p.sessions)), change: sessions_change(r, p) }
                            }
                            td { class: "r muted", "data-label": "비중", {format::pct(if total_sessions > 0 { r.sessions as f64 / total_sessions as f64 } else { 0.0 }, 1)} }
                            td { class: "r", "data-label": "방문자", "{format::count(r.visitors)}" }
                            if compare {
                                CmpCells {
                                    prev: None,
                                    change: change(r.visitors as f64, p.as_ref().map(|p| p.visitors as f64)),
                                    change_label: "방문자 변화",
                                    change_only: true,
                                }
                            }
                            td { class: "r m-hide", "data-label": "페이지뷰", "{format::count(r.pageviews)}" }
                            td { class: "r", "data-label": "세션당 페이지뷰", "{per(r.pageviews, r.sessions)}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn Acquisition(pid: ReadSignal<i32>) -> Element {
    let ctx = use_app();
    let sort = use_signal(|| Sort::desc(Col::Sessions));
    let res = use_resource(move || {
        let (p, r) = (pid(), ctx.range());
        async move { api::get::<Vec<ChannelTrafficRow>>(&query::report(p, Report::Channels, r)).await }
    });
    let prev_res =
        use_previous::<Vec<ChannelTrafficRow>>(move |r| query::report(pid(), Report::Channels, r));
    let prev = loaded_prev(&prev_res);
    let range = ctx.range();

    rsx! {
        PageHead { title: "유입", desc: format!("{} · 세션을 시작한 채널 기준", range.label()) }
        {load(res, rsx! { Card { SkeletonBlock { rows: 8 } } }, |rows| {
            if rows.is_empty() {
                return rsx! {
                    Card { EmptyState { icon: crate::components::icon::IconName::Acquisition, title: "이 기간에 유입이 없어요", desc: "기간을 넓히거나 설치 상태를 확인해 주세요." } }
                };
            }
            let compare = prev.is_some();
            let paired = pair_channels(&rows, prev.as_deref());
            let total_sessions: u64 = rows.iter().map(|r| r.sessions).sum();
            let items: Vec<(String, i64)> = rows.iter().map(|r| (labels::channel(&r.channel), r.sessions as i64)).collect();
            let bars: Vec<(String, i64, Option<i64>)> = paired.iter().map(|(r, p)| (labels::channel(&r.channel), r.sessions as i64, p.as_ref().map(|p| p.sessions as i64))).collect();
            let bars = top_with_rest_prev(&bars, 7);
            let csv_rows: Vec<Vec<String>> = rows.iter().map(|r| vec![labels::channel(&r.channel), r.channel.clone(), r.sessions.to_string(), r.visitors.to_string(), r.pageviews.to_string()]).collect();
            rsx! {
                div { class: "grid grid-2",
                    Card { title: "채널 구성",
                        ShareBar { parts: top_with_rest(&items, 4) }
                        div { class: "callout", style: "margin-top: 20px",
                            strong { "{items.iter().max_by_key(|i| i.1).map(|i| i.0.clone()).unwrap_or_default()}" }
                            "이(가) 세션을 가장 많이 데려왔어요. 전체 세션 {format::count(total_sessions)}개."
                            if let Some(p) = &prev {
                                {
                                    let pt: u64 = p.iter().map(|r| r.sessions).sum();
                                    rsx! { " 이전 기간 {format::count(pt)}개 대비 " ChangeTag { change: change(total_sessions as f64, Some(pt as f64)) } }
                                }
                            }
                        }
                    }
                    Card { title: "채널별 세션",
                        BarList {
                            items: bars.iter().map(|b| (b.0.clone(), b.1)).collect::<Vec<_>>(),
                            show_share: true,
                            prev: compare.then(|| bars.iter().map(|b| b.2).collect::<Vec<_>>()),
                        }
                    }
                }
                Card {
                    title: "채널 상세",
                    actions: rsx! { CsvButton { file_name: format!("유입_{}.csv", range.label()), headers: vec!["채널".into(), "채널 ID".into(), "세션".into(), "방문자".into(), "페이지뷰".into()], rows: csv_rows } },
                    ChannelTable { rows: paired, compare, sort }
                }
            }
        })}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(c: &str, s: u64) -> ChannelTrafficRow {
        ChannelTrafficRow {
            channel: c.into(),
            sessions: s,
            entrances: s,
            visitors: s,
            pageviews: s,
        }
    }

    #[test]
    fn channels_pair_with_new_and_gone() {
        let p = pair_channels(&[r("a", 3), r("b", 1)], Some(&[r("a", 2), r("c", 4)]));
        assert_eq!(p.len(), 3);
        assert_eq!(sessions_change(&p[0].0, &p[0].1), Change::Pct(0.5));
        assert_eq!(sessions_change(&p[1].0, &p[1].1), Change::New);
        assert_eq!(p[2].0.channel, "c");
        assert_eq!(sessions_change(&p[2].0, &p[2].1), Change::Pct(-1.0));
        // 비교 끔: 이전 값 없음, 사라진 행도 없음
        let off = pair_channels(&[r("a", 3)], None);
        assert_eq!(off, vec![(r("a", 3), None)]);
    }
}

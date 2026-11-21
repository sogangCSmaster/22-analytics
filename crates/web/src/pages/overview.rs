//! 개요: 핵심 지표(이전 기간 대비) → 방문 추이 → 유입 채널 구성 · 많이 본 페이지.

use crate::Route;
use crate::api;
use crate::components::chart::{BarList, LineChart, Series, ShareBar};
use crate::components::ui::{
    Card, EmptyState, SegmentedControl, SkeletonBlock, SkeletonCards, StatCard, StatStyle, load,
    load_state,
};
use crate::format;
use crate::logic::dates::Day;
use crate::logic::{labels, query, query::Report};
use crate::state::use_app;
use analytics_core::dto::reports::{
    ChannelTrafficRow, OverviewReport, PageRow, TimeseriesPoint, TimeseriesReport,
};
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Metric {
    Visitors,
    Sessions,
    Pageviews,
}

impl Metric {
    pub fn label(self) -> &'static str {
        match self {
            Metric::Visitors => "방문자",
            Metric::Sessions => "세션",
            Metric::Pageviews => "페이지뷰",
        }
    }

    pub fn of(self, p: &TimeseriesPoint) -> f64 {
        (match self {
            Metric::Visitors => p.visitors,
            Metric::Sessions => p.sessions,
            Metric::Pageviews => p.pageviews,
        }) as f64
    }
}

/// 채널 구성: 상위 n개 + 나머지는 "기타".
pub fn top_with_rest(rows: &[(String, i64)], n: usize) -> Vec<(String, i64)> {
    let mut v: Vec<(String, i64)> = rows.to_vec();
    v.sort_by_key(|b| std::cmp::Reverse(b.1));
    if v.len() <= n + 1 {
        return v;
    }
    let rest: i64 = v[n..].iter().map(|r| r.1).sum();
    v.truncate(n);
    v.push(("기타".into(), rest));
    v
}

#[component]
pub fn Overview(pid: ReadSignal<i32>) -> Element {
    let ctx = use_app();
    let mut metric = use_signal(|| Metric::Visitors);

    let overview = use_resource(move || {
        let (p, r) = (pid(), ctx.range());
        async move { api::get::<OverviewReport>(&query::report(p, Report::Overview, r)).await }
    });
    // 선택 기간과 이전 기간 추이는 따로(동시에) 부른다. 비교를 켜고 끌 때는 이전 기간만 다시 부른다.
    let cur_series = use_resource(move || {
        let (p, r) = (pid(), ctx.range());
        async move { api::get::<TimeseriesReport>(&query::report(p, Report::Timeseries, r)).await }
    });
    let prev_series = use_resource(move || {
        let (p, r, cmp) = (pid(), ctx.range(), ctx.compare());
        async move {
            if !cmp {
                return Ok(None);
            }
            api::get::<TimeseriesReport>(&query::report(p, Report::Timeseries, r.previous()))
                .await
                .map(Some)
        }
    });
    // 두 결과를 합쳐 기존처럼 (현재, 이전)으로 그린다. 하나라도 로딩 중이면 로딩, 오류면 오류.
    let series = use_memo(move || {
        let cur = cur_series.read().clone()?;
        let prev = prev_series.read().clone()?;
        Some(cur.and_then(|c| prev.map(|p| (c, p))))
    });
    let channels = use_resource(move || {
        let (p, r) = (pid(), ctx.range());
        async move { api::get::<Vec<ChannelTrafficRow>>(&query::report(p, Report::Channels, r)).await }
    });
    let pages = use_resource(move || {
        let (p, r) = (pid(), ctx.range());
        async move { api::get::<Vec<PageRow>>(&query::pages(p, r, "", 8)).await }
    });

    let range = ctx.range();
    let spark = |m: Metric| -> Option<Vec<f64>> {
        match &*series.read() {
            Some(Ok((cur, _))) if cur.points.len() > 1 => {
                Some(cur.points.iter().map(|p| m.of(p)).collect())
            }
            _ => None,
        }
    };
    let (sv, ss, sp) = (
        spark(Metric::Visitors),
        spark(Metric::Sessions),
        spark(Metric::Pageviews),
    );

    rsx! {
        crate::components::ui::PageHead {
            title: "개요",
            desc: format!("{} · 이전 {}일과 비교", range.label(), range.days()),
        }

        {load(overview, rsx! { SkeletonCards {} }, |o| {
            let d = |c: u64, p: u64| crate::logic::compare::change(c as f64, Some(p as f64));
            let empty = o.current.events == 0 && o.previous.events == 0;
            rsx! {
                if empty {
                    Card { filled: true,
                        EmptyState {
                            icon: crate::components::icon::IconName::Realtime,
                            title: "아직 받은 데이터가 없어요",
                            desc: "사이트에 스크립트를 설치하면 몇 초 안에 방문 기록이 나타나요. 설치했다면 실시간 화면에서 확인해 보세요.",
                            action: rsx! {
                                Link { class: "btn btn-primary", to: Route::Settings { pid: pid(), tab: "install".into() }, "설치 코드 복사하기" }
                            },
                        }
                    }
                }
                div { class: "grid grid-4",
                    StatCard { label: "방문자", value: format::count(o.current.visitors), change: d(o.current.visitors, o.previous.visitors), sub: format!("이전 {}", format::compact(o.previous.visitors)), spark: sv.clone() }
                    StatCard { label: "세션", value: format::count(o.current.sessions), change: d(o.current.sessions, o.previous.sessions), sub: format!("이전 {}", format::compact(o.previous.sessions)), spark: ss.clone() }
                    StatCard { label: "페이지뷰", value: format::count(o.current.pageviews), change: d(o.current.pageviews, o.previous.pageviews), sub: format!("세션당 {}", per(o.current.pageviews, o.current.sessions)), spark: sp.clone() }
                    StatCard {
                        label: "매출",
                        value: format::won_short(o.current.revenue),
                        change: crate::logic::compare::change(o.current.revenue as f64, Some(o.previous.revenue as f64)),
                        sub: format!("주문 {}건", format::count(o.current.orders)),
                        style: StatStyle::Plain,
                    }
                }
            }
        })}

        Card {
            title: "방문 추이",
            actions: rsx! {
                SegmentedControl {
                    options: [Metric::Visitors, Metric::Sessions, Metric::Pageviews].into_iter().map(|m| (m, m.label().to_string())).collect::<Vec<_>>(),
                    value: metric(),
                    onchange: move |m| metric.set(m),
                }
            },
            {load_state(series(), rsx! { SkeletonBlock { chart: true } }, |(cur, prev)| {
                let m = metric();
                let days: Vec<Day> = cur.points.iter().filter_map(|p| Day::parse(&p.date)).collect();
                let mut s = vec![Series { name: format!("{} (선택 기간)", m.label()), values: cur.points.iter().map(|p| m.of(p)).collect(), muted: false }];
                if let Some(prev) = prev {
                    s.push(Series { name: "이전 기간".into(), values: prev.points.iter().map(|p| m.of(p)).collect(), muted: true });
                }
                let total: f64 = s[0].values.iter().sum();
                rsx! {
                    if cur.points.is_empty() {
                        EmptyState { icon: crate::components::icon::IconName::Overview, title: "표시할 기간이 없어요", desc: "기간을 다시 선택해 주세요." }
                    } else {
                        LineChart {
                            series: s,
                            labels: days.iter().map(|d| d.short()).collect::<Vec<_>>(),
                            tooltip_titles: days.iter().map(|d| d.long_kr()).collect::<Vec<_>>(),
                            aria_label: format!("{} 일별 추이, 합계 {}", m.label(), format::int(total as i64)),
                        }
                    }
                }
            }, move || {
                let (mut c, mut p) = (cur_series, prev_series);
                c.restart();
                p.restart();
            })}
        }

        div { class: "grid grid-2",
            Card {
                title: "유입 채널",
                actions: rsx! { Link { class: "link", to: Route::Acquisition { pid: pid() }, "전체 보기 →" } },
                {load(channels, rsx! { SkeletonBlock { rows: 5 } }, |rows| {
                    let items: Vec<(String, i64)> = rows.iter().map(|r| (labels::channel(&r.channel), r.sessions as i64)).collect();
                    rsx! {
                        if rows.is_empty() {
                            EmptyState { icon: crate::components::icon::IconName::Acquisition, title: "유입 기록이 없어요", desc: "이 기간에 시작된 세션이 없습니다." }
                        } else {
                            ShareBar { parts: top_with_rest(&items, 4) }
                            div { style: "margin-top: 20px",
                                BarList { items: top_with_rest(&items, 6), show_share: true }
                            }
                        }
                    }
                })}
            }
            Card {
                title: "많이 본 페이지",
                actions: rsx! { Link { class: "link", to: Route::Pages { pid: pid() }, "전체 보기 →" } },
                {load(pages, rsx! { SkeletonBlock { rows: 6 } }, |rows| rsx! {
                    if rows.is_empty() {
                        EmptyState { icon: crate::components::icon::IconName::Pages, title: "페이지 조회가 없어요", desc: "이 기간에 기록된 페이지뷰가 없습니다." }
                    } else {
                        div { class: "table-wrap",
                            table { class: "table",
                                thead { tr { th { scope: "col", "페이지" } th { class: "r", scope: "col", "페이지뷰" } th { class: "r", scope: "col", "방문자" } } }
                                tbody {
                                    for (i, r) in rows.iter().enumerate() {
                                        tr { key: "{i}",
                                            td { class: "path-cell", title: "{format::path(&r.path)}", "{format::path(&r.path)}" }
                                            td { class: "r", "{format::count(r.pageviews)}" }
                                            td { class: "r muted", "{format::count(r.visitors)}" }
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
}

/// a / b (소수 1자리), b가 0이면 0
pub fn per(a: u64, b: u64) -> String {
    if b == 0 {
        "0".into()
    } else {
        format::trim_float(a as f64 / b as f64, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_small_channels_into_rest() {
        let rows: Vec<(String, i64)> = (1..=7).map(|i| (format!("c{i}"), i)).collect();
        let v = top_with_rest(&rows, 4);
        assert_eq!(v.len(), 5);
        assert_eq!(v[0], ("c7".into(), 7));
        assert_eq!(v[4], ("기타".into(), 1 + 2 + 3));
        // 하나만 남으면 "기타"로 묶지 않는다
        assert_eq!(top_with_rest(&rows[..5], 4).len(), 5);
        assert!(top_with_rest(&[], 4).is_empty());
    }

    #[test]
    fn per_session() {
        assert_eq!(per(10, 4), "2.5");
        assert_eq!(per(10, 0), "0");
    }
}

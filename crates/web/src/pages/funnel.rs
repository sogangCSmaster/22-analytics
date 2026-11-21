//! 전자상거래 퍼널: 상품 조회 → 장바구니 → 결제 시작 → 구매 단계별 전환.

use crate::Route;
use crate::api;
use crate::components::chart::FunnelChart;
use crate::components::ui::{Card, EmptyState, PageHead, SkeletonBlock, StatCard, load};
use crate::format;
use crate::logic::{labels, query, query::Report};
use crate::state::use_app;
use analytics_core::dto::reports::{FunnelReport, FunnelStep};
use dioxus::prelude::*;

/// 이전 단계 대비 이탈이 가장 큰 단계 (첫 단계 제외): (인덱스, 이탈률)
pub fn biggest_drop(steps: &[FunnelStep]) -> Option<(usize, f64)> {
    steps
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(i, _)| steps[i - 1].visitors > 0)
        .map(|(i, s)| (i, 1.0 - s.rate_from_prev))
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

fn step_name(e: &str) -> String {
    labels::event_hint(e)
        .map(str::to_owned)
        .unwrap_or_else(|| e.to_owned())
}

#[component]
pub fn FunnelTable(steps: Vec<(FunnelStep, Option<FunnelStep>)>, compare: bool) -> Element {
    let _ = (steps, compare);
    rsx! {}
}

#[component]
pub fn Funnel(pid: ReadSignal<i32>) -> Element {
    let ctx = use_app();
    let res = use_resource(move || {
        let (p, r) = (pid(), ctx.range());
        async move { api::get::<FunnelReport>(&query::report(p, Report::Funnel, r)).await }
    });
    let range = ctx.range();
    rsx! {
        PageHead { title: "전자상거래", desc: format!("{} · 단계를 순서대로 거친 순 방문자 기준", range.label()) }
        {load(res, rsx! { Card { SkeletonBlock { rows: 5 } } }, |f| {
            let first = f.steps.first().map(|s| s.visitors).unwrap_or(0);
            if first == 0 {
                return rsx! {
                    Card {
                        EmptyState {
                            icon: crate::components::icon::IconName::Funnel,
                            title: "전자상거래 이벤트가 없어요",
                            desc: "설정에서 '전자상거래'를 켜고 view_item · add_to_cart · begin_checkout · purchase 이벤트를 보내면 단계별 전환이 나타나요.",
                            action: rsx! { Link { class: "btn btn-secondary", to: Route::Settings { pid: pid(), tab: "tracking".into() }, "수집 설정 열기" } },
                        }
                    }
                };
            }
            let last = f.steps.last().cloned();
            let drop = biggest_drop(&f.steps);
            rsx! {
                div { class: "grid grid-3",
                    StatCard { label: "첫 단계 방문자", value: format::count(first), sub: step_name(&f.steps[0].event_name) }
                    StatCard { label: "최종 전환율", value: format::pct(last.as_ref().map(|s| s.rate_from_first).unwrap_or(0.0), 1), sub: format!("{}명 {}", format::count(last.as_ref().map(|s| s.visitors).unwrap_or(0)), last.as_ref().map(|s| step_name(&s.event_name)).unwrap_or_default()) }
                    if let Some((i, rate)) = drop {
                        StatCard { label: "가장 많이 빠지는 곳", value: format::pct(rate, 1), sub: format!("{} → {}", step_name(&f.steps[i - 1].event_name), step_name(&f.steps[i].event_name)) }
                    }
                }
                Card { title: "단계별 전환",
                    FunnelChart { steps: f.steps.iter().map(|s| (step_name(&s.event_name), s.visitors, s.rate_from_prev, s.rate_from_first)).collect::<Vec<_>>() }
                }
                Card { title: "단계 상세",
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr {
                                th { scope: "col", "단계" }
                                th { class: "r", scope: "col", "방문자" }
                                th { class: "r", scope: "col", "이전 단계 대비" }
                                th { class: "r", scope: "col", "첫 단계 대비" }
                                th { class: "r", scope: "col", "이탈" }
                            } }
                            tbody {
                                for (i, s) in f.steps.iter().enumerate() {
                                    tr { key: "{i}",
                                        td {
                                            div { class: "cell-title", "{i + 1}. {step_name(&s.event_name)}" }
                                            div { class: "cell-sub mono", "{s.event_name}" }
                                        }
                                        td { class: "r", "{format::count(s.visitors)}" }
                                        td { class: "r", {if i == 0 { "–".to_string() } else { format::pct(s.rate_from_prev, 1) }} }
                                        td { class: "r", {format::pct(s.rate_from_first, 1)} }
                                        td { class: "r muted", {if i == 0 { "–".to_string() } else { format::count(f.steps[i - 1].visitors.saturating_sub(s.visitors)) + "명" }} }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: u64, prev: f64) -> FunnelStep {
        FunnelStep {
            event_name: "x".into(),
            visitors: v,
            rate_from_prev: prev,
            rate_from_first: 0.0,
        }
    }

    #[test]
    fn finds_biggest_drop() {
        assert_eq!(biggest_drop(&[]), None);
        assert_eq!(biggest_drop(&[s(10, 1.0)]), None);
        let steps = [s(100, 1.0), s(50, 0.5), s(10, 0.2), s(9, 0.9)];
        assert_eq!(biggest_drop(&steps).map(|d| d.0), Some(2));
        // 이전 단계가 0명이면 이탈률을 따지지 않는다
        assert_eq!(biggest_drop(&[s(0, 1.0), s(0, 0.0)]), None);
    }
}

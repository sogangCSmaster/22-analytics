//! 실제 매출(중복 소거) 리포트. 매뉴얼 1.C·1.D 화면.

use crate::components::chart::{BarList, SplitBar};
use crate::components::ui::{
    Badge, Button, Card, Drawer, SegmentedControl, Size, StatCard, StatStyle, Toast, Tone, Variant,
    copy_to_clipboard, sleep_ms,
};
use crate::format;
use crate::mock::{self, ChannelRevenue};
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Model {
    LastClick,
    FirstClick,
    Linear,
    TimeDecay,
}

impl Model {
    fn label(self) -> &'static str {
        match self {
            Model::LastClick => "마지막 클릭",
            Model::FirstClick => "첫 클릭",
            Model::Linear => "균등 배분",
            Model::TimeDecay => "시간 가중",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Model::LastClick => {
                "구매 직전 클릭에 매출을 모두 줍니다. 단기 효율을 빠르게 판단할 때 씁니다."
            }
            Model::FirstClick => {
                "처음 데려온 매체에 매출을 모두 줍니다. 신규 고객 확보가 목표일 때 씁니다."
            }
            Model::Linear => "거쳐 간 매체에 똑같이 나눕니다. 여러 매체를 함께 운영할 때 씁니다.",
            Model::TimeDecay => {
                "구매에 가까운 클릭일수록 더 줍니다. 오래 고민하는 고관여 상품에 씁니다."
            }
        }
    }

    fn amount(self, c: &ChannelRevenue) -> i64 {
        match self {
            Model::LastClick => c.last_click,
            Model::FirstClick => c.first_click,
            Model::Linear => c.linear,
            Model::TimeDecay => c.time_decay,
        }
    }
}

const STANDARD_MESSAGE: &str = "기존 매체별 리포트를 합산한 ROAS 500%가 325%로 낮아진 것은 성과가 떨어진 것이 아닙니다. 동일 주문이 네이버와 메타에 중복으로 집계되던 35%를 주문번호 기준으로 소거하여 자사몰 실매출과 일치시킨 결과입니다. 이제 실매출 기준으로 중복 비용을 줄이고 순증 매출이 큰 매체에 예산을 집중할 수 있습니다.";

#[component]
pub fn Revenue() -> Element {
    let mut model = use_signal(|| Model::LastClick);
    let mut lookback = use_signal(|| 7u8);
    let mut drawer = use_signal(|| None::<ChannelRevenue>);
    let mut toast = use_signal(|| None::<String>);

    let rows = mock::channel_revenue();
    let raw_total: i64 = rows.iter().map(|r| r.raw).sum();
    let real_total: i64 = rows.iter().map(|r| model().amount(r)).sum();
    let spend_total: i64 = rows.iter().map(|r| r.spend).sum();
    let dup = raw_total - real_total;
    let dup_rate = dup as f64 / raw_total as f64;
    let roas = |rev: i64| {
        if spend_total > 0 {
            format::pct(rev as f64 / spend_total as f64, 0)
        } else {
            "-".into()
        }
    };
    let overlaps: Vec<(String, i64)> = mock::overlap_pairs()
        .into_iter()
        .map(|(l, v)| (l.to_string(), v))
        .collect();

    rsx! {
        div { class: "page-head",
            div {
                h1 { class: "page-title", "실제 매출" }
                p { class: "page-desc", "주문번호 기준으로 한 번만 센 매출을 매체별로 나눴습니다." }
            }
            div { class: "row",
                SegmentedControl {
                    options: vec![(7u8, "7일 이내 클릭".to_string()), (30u8, "30일 이내 클릭".to_string())],
                    value: lookback(),
                    onchange: move |v| lookback.set(v),
                }
            }
        }

        div { class: "grid grid-3",
            StatCard {
                label: "매체 합산 매출",
                value: format::won_short(raw_total),
                sub: format!("매체 대시보드를 단순 합산 · ROAS {}", roas(raw_total)),
                style: StatStyle::Struck,
            }
            StatCard {
                label: "실제 매출",
                value: format::won_short(real_total),
                sub: format!("주문번호 기준 1회 집계 · ROAS {}", roas(real_total)),
                style: StatStyle::Hero,
            }
            StatCard {
                label: "매체 간 중복률",
                value: format::pct(dup_rate, 0),
                sub: format!("{} 중복 제거", format::won_short(dup)),
            }
        }

        Card {
            title: "매체별 매출 기여",
            actions: rsx! {
                SegmentedControl {
                    options: [Model::LastClick, Model::FirstClick, Model::Linear, Model::TimeDecay]
                        .into_iter()
                        .map(|m| (m, m.label().to_string()))
                        .collect::<Vec<_>>(),
                    value: model(),
                    onchange: move |m| model.set(m),
                }
            },
            div { class: "callout", style: "margin-bottom: 16px;",
                strong { "{model().label()}" }
                " · {model().help()} 어떤 방식으로 나눠도 합계는 실제 매출과 같습니다."
            }
            SplitBar {
                parts: rows
                    .iter()
                    .zip(["var(--chart-1)", "var(--chart-2)", "var(--chart-3)", "var(--chart-4)", "var(--chart-5)"])
                    .map(|(r, c)| (r.label.to_string(), model().amount(r), c.to_string()))
                    .collect::<Vec<_>>(),
            }
            div { class: "table-wrap", style: "margin-top: 16px;",
                table { class: "table",
                    thead {
                        tr {
                            th { "매체" }
                            th { class: "r", "매체가 주장한 매출" }
                            th { class: "r", "기여 매출" }
                            th { class: "r", "비중" }
                            th { class: "r", "어시스트" }
                            th { class: "r", "광고비" }
                            th { class: "r", "ROAS" }
                        }
                    }
                    tbody {
                        for r in rows.clone() {
                            tr {
                                key: "{r.key}",
                                class: "clickable",
                                onclick: {
                                    let r = r.clone();
                                    move |_| drawer.set(Some(r.clone()))
                                },
                                td { "{r.label}" }
                                td { class: "r muted", {format::won(r.raw)} }
                                td { class: "r", strong { {format::won(model().amount(&r))} } }
                                td { class: "r", {format::pct(model().amount(&r) as f64 / real_total as f64, 1)} }
                                td { class: "r", {format::int(r.assists as i64)} }
                                td { class: "r", if r.spend > 0 { {format::won(r.spend)} } else { span { class: "muted", "-" } } }
                                td { class: "r",
                                    if r.spend > 0 {
                                        {format::pct(model().amount(&r) as f64 / r.spend as f64, 0)}
                                    } else {
                                        span { class: "muted", "-" }
                                    }
                                }
                            }
                        }
                    }
                    tfoot {
                        tr {
                            td { "합계" }
                            td { class: "r muted", {format::won(raw_total)} }
                            td { class: "r", {format::won(real_total)} }
                            td { class: "r", "100%" }
                            td { class: "r", "" }
                            td { class: "r", {format::won(spend_total)} }
                            td { class: "r", {roas(real_total)} }
                        }
                    }
                }
            }
        }

        div { class: "grid grid-2",
            Card { title: "중복이 많은 매체 조합 (주문 수)",
                BarList { items: overlaps }
            }
            Card {
                title: "광고주에게는 이렇게 설명하세요",
                actions: rsx! {
                    Button {
                        variant: Variant::Secondary,
                        size: Size::Sm,
                        onclick: move |_| {
                            copy_to_clipboard(STANDARD_MESSAGE);
                            toast.set(Some("설명 문안을 복사했어요".into()));
                            spawn(async move {
                                sleep_ms(1800).await;
                                toast.set(None);
                            });
                        },
                        "복사"
                    }
                },
                p { class: "quote", "“{STANDARD_MESSAGE}”" }
            }
        }

        Drawer {
            open: drawer().is_some(),
            title: drawer().map(|r| r.label.to_string()).unwrap_or_default(),
            onclose: move |_| drawer.set(None),
            if let Some(r) = drawer() {
                div { class: "grid grid-2",
                    StatCard { label: "기여 매출", value: format::won_short(model().amount(&r)) }
                    StatCard { label: "매체가 주장한 매출", value: format::won_short(r.raw), style: StatStyle::Struck }
                }
                div {
                    div { class: "spread", style: "margin-bottom: 12px;",
                        strong { "주문 예시 · ORD-9901" }
                        Badge { tone: Tone::Primary, "100,000원" }
                    }
                    div { class: "timeline",
                        TimelineItem { time: "18:10", title: "네이버 검색광고 클릭", desc: "마지막 클릭 0원 · 균등 50,000원" }
                        TimelineItem { time: "18:40", title: "메타 리타게팅 클릭", desc: "마지막 클릭 100,000원 · 균등 50,000원" }
                        TimelineItem { time: "18:45", title: "결제 완료 (주문 서버 전송)", desc: "주문번호로 한 번만 확정", primary: true }
                    }
                }
                div { class: "callout",
                    "두 매체 대시보드는 각각 100,000원씩 보고하지만, 실제 결제는 한 번입니다. "
                    strong { "합계는 어떤 모델에서도 100,000원" }
                    "으로 같습니다."
                }
            }
        }

        if let Some(msg) = toast() {
            Toast { message: msg }
        }
    }
}

#[component]
fn TimelineItem(
    time: String,
    title: String,
    desc: String,
    #[props(default)] primary: bool,
) -> Element {
    rsx! {
        div { class: "tl-item",
            div { class: if primary { "tl-dot primary" } else { "tl-dot" } }
            div {
                div { class: "tl-time", "{time}" }
                div { class: "tl-title", "{title}" }
                div { class: "tl-time", "{desc}" }
            }
        }
    }
}

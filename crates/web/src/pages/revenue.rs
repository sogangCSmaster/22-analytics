//! 실제 매출(중복 소거): 매체 합산 vs 실제 매출 vs 중복률, 모델·lookback 선택, 매체별 기여·ROAS,
//! 매체 조합 경로, 최근 주문 → 주문 상세(접점 타임라인·모델별 배분) 패널.

use crate::Route;
use crate::api;
use crate::components::chart::{BarList, ShareBar};
use crate::components::ui::{
    Badge, Button, Card, ChangeTag, Drawer, EmptyState, PageHead, SegmentedControl, Size,
    SkeletonBlock, SkeletonCards, StatCard, StatStyle, Tone, Variant, copy_to_clipboard, load,
};
use crate::format;
use crate::logic::compare::{Change, change, pair, pp};
use crate::logic::view::{is_activation_key, pager, scoped};
use crate::logic::{dates, labels, query};
use crate::state::{loaded_prev, use_app};
use analytics_core::attribution::Model;
use analytics_core::dto::admin::AttributionSettingsDto;
use analytics_core::dto::reports::{
    ChannelRevenueRow, OrderDetail, OverlapReport, RecentOrdersReport, RevenueReport,
};
use dioxus::prelude::*;

pub const ORDERS_PAGE: u32 = 10;

/// 광고주 설명용 문안 (매뉴얼 1.D(3) 톤). 실제 수치로 채운다.
pub fn explain(r: &RevenueReport) -> Option<String> {
    if r.raw_sum <= 0 || r.duplicate_amount <= 0 {
        return None;
    }
    let roas_part = match (r.spend_total > 0).then(|| r.raw_sum as f64 / r.spend_total as f64) {
        Some(raw_roas) => format!(
            "기존 매체별 리포트를 합산한 ROAS {}가 {}로 낮아진 것은 성과가 떨어진 것이 아닙니다. ",
            format::roas(Some(raw_roas)),
            format::roas(r.roas)
        ),
        None => format!(
            "매체 리포트를 합산한 매출 {}이 실제 매출 {}로 줄어든 것은 성과가 떨어진 것이 아닙니다. ",
            format::won_short(r.raw_sum),
            format::won_short(r.deduplicated)
        ),
    };
    Some(format!(
        "{roas_part}같은 주문이 여러 매체에 중복으로 집계되던 {}를 주문번호 기준으로 소거하여 자사몰 실매출과 일치시킨 결과입니다. 이제 실매출 기준으로 중복 비용을 줄이고 순증 매출이 큰 매체에 예산을 집중할 수 있습니다.",
        format::pct(r.duplicate_rate, 0)
    ))
}

pub fn path_label(channels: &[String]) -> String {
    channels
        .iter()
        .map(|c| labels::channel(c))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// 기여 설정을 읽었는데 클릭 인정 기간이 하나도 없으면, 오류 대신 설정으로 안내한다.
pub fn needs_lookback_setup(settings_ready: bool, lookbacks: &[u8]) -> bool {
    settings_ready && lookbacks.is_empty()
}

type SettingsRes = Resource<api::ApiResult<AttributionSettingsDto>>;

/// 선택된 모델·lookback (사용자가 고르지 않았으면 프로젝트 기본값) 과 선택 가능한 lookback 목록.
fn pick(
    settings: SettingsRes,
    model: Signal<Option<Model>>,
    lookback: Signal<Option<u8>>,
) -> (Model, Option<u8>, Vec<u8>) {
    let (lookbacks, default_model) = match &*settings.read() {
        Some(Ok(s)) => (
            s.lookback_days
                .iter()
                .filter_map(|d| u8::try_from(*d).ok())
                .collect::<Vec<_>>(),
            s.default_model,
        ),
        _ => (vec![], Model::LastClick),
    };
    let m = model().unwrap_or(if default_model.conserves_revenue() {
        default_model
    } else {
        Model::LastClick
    });
    let lb = lookback()
        .filter(|l| lookbacks.contains(l))
        .or(lookbacks.first().copied());
    (m, lb, lookbacks)
}

/// 이전 기간 보고서가 같은 기준(모델·lookback)일 때만 비교한다 (모델을 막 바꿨을 때 이전 모델 값과 섞지 않는다).
pub fn same_basis(cur: &RevenueReport, prev: Option<RevenueReport>) -> Option<RevenueReport> {
    prev.filter(|p| p.model == cur.model && p.lookback_days == cur.lookback_days)
}

/// 현재·이전 매체별 기여를 매체로 맞춘다 (매체 목록은 잘리지 않는다).
pub fn pair_channel_revenue(
    cur: &[ChannelRevenueRow],
    prev: Option<&[ChannelRevenueRow]>,
) -> Vec<(ChannelRevenueRow, Option<ChannelRevenueRow>)> {
    match prev {
        Some(p) => pair(
            cur,
            p,
            |r| r.channel.clone(),
            |r| ChannelRevenueRow {
                channel: r.channel.clone(),
                raw_claimed: 0,
                attributed: 0,
                share: 0.0,
                assists: 0,
                spend: 0,
                roas: None,
            },
            true,
            true,
        ),
        None => cur.iter().map(|r| (r.clone(), None)).collect(),
    }
}

/// ROAS 변화 (%p). 어느 한쪽이라도 광고비가 없으면 비교하지 않는다.
fn roas_change(cur: Option<f64>, prev: Option<Option<f64>>) -> Change {
    match (cur, prev.flatten()) {
        (Some(c), Some(p)) => pp(c, Some(p)),
        _ => Change::Unknown,
    }
}

/// 매체별 매출 기여 표. 비교를 켜면 기여 매출의 이전 값·변화와 ROAS 변화(%p)가 붙는다.
/// 모바일 카드 행에서는 매체·기여 매출·비중·ROAS만 남긴다.
#[component]
pub fn ChannelRevenueTable(report: RevenueReport, prev: Option<RevenueReport>) -> Element {
    let r = report;
    let compare = prev.is_some();
    let rows = pair_channel_revenue(&r.channels, prev.as_ref().map(|p| p.channels.as_slice()));
    let attr_change = |c: &ChannelRevenueRow, p: &Option<ChannelRevenueRow>| {
        change(c.attributed as f64, p.as_ref().map(|p| p.attributed as f64))
    };
    let total_change = change(
        r.deduplicated as f64,
        prev.as_ref().map(|p| p.deduplicated as f64),
    );
    let total_roas_change = roas_change(r.roas, prev.as_ref().map(|p| p.roas));
    rsx! {
        div { class: "table-wrap", style: "margin-top: 16px;",
            table { class: "table table-cards",
                thead { tr {
                    th { scope: "col", "매체" }
                    th { class: "r m-hide", scope: "col", "매체가 주장한 매출" }
                    th { class: "r", scope: "col", "기여 매출" }
                    if compare {
                        th { class: "r cmp-col m-hide", scope: "col", "이전 기간" }
                        th { class: "r cmp-col m-hide", scope: "col", "변화" }
                    }
                    th { class: "r", scope: "col", "비중" }
                    th { class: "r m-hide", scope: "col", "어시스트" }
                    th { class: "r m-hide", scope: "col", "광고비" }
                    th { class: "r", scope: "col", "ROAS" }
                    if compare {
                        th { class: "r cmp-col m-hide", scope: "col", "ROAS 변화" }
                    }
                } }
                tbody {
                    for (c, p) in rows.iter() {
                        tr { key: "{c.channel}",
                            td { "data-label": "매체", div { class: "cell-title", "{labels::channel(&c.channel)}" } }
                            td { class: "r muted strike-soft m-hide", "data-label": "매체가 주장한 매출", "{format::won(c.raw_claimed)}" }
                            td { class: "r strong main-metric", "data-label": "기여 매출",
                                "{format::won(c.attributed)}"
                                if compare {
                                    ChangeTag { change: attr_change(c, p), class: "m-only" }
                                }
                            }
                            if compare {
                                td { class: "r muted cmp-prev m-hide", "data-label": "이전 기간", {p.as_ref().map(|p| format::won(p.attributed)).unwrap_or_else(|| "–".into())} }
                                td { class: "r cmp-chg m-hide", "data-label": "변화", ChangeTag { change: attr_change(c, p) } }
                            }
                            td { class: "r", "data-label": "비중", {format::pct(c.share, 1)} }
                            td { class: "r m-hide", "data-label": "어시스트", "{format::count(c.assists)}" }
                            td { class: "r m-hide", "data-label": "광고비", {if c.spend > 0 { format::won(c.spend) } else { "–".into() }} }
                            td { class: "r main-metric", "data-label": "ROAS",
                                {format::roas(c.roas)}
                                if compare {
                                    ChangeTag { change: roas_change(c.roas, p.as_ref().map(|p| p.roas)), class: "m-only" }
                                }
                            }
                            if compare {
                                td { class: "r cmp-chg m-hide", "data-label": "ROAS 변화", ChangeTag { change: roas_change(c.roas, p.as_ref().map(|p| p.roas)) } }
                            }
                        }
                    }
                }
                tfoot { tr {
                    td { "data-label": "매체", "합계" }
                    td { class: "r m-hide", "data-label": "매체가 주장한 매출", "{format::won(r.raw_sum)}" }
                    td { class: "r main-metric", "data-label": "기여 매출",
                        "{format::won(r.deduplicated)}"
                        if compare {
                            ChangeTag { change: total_change, class: "m-only" }
                        }
                    }
                    if compare {
                        td { class: "r cmp-prev m-hide", "data-label": "이전 기간", {prev.as_ref().map(|p| format::won(p.deduplicated)).unwrap_or_default()} }
                        td { class: "r cmp-chg m-hide", "data-label": "변화", ChangeTag { change: total_change } }
                    }
                    td { class: "r", "data-label": "비중", "100%" }
                    td { class: "r m-hide", "" }
                    td { class: "r m-hide", "data-label": "광고비", {if r.spend_total > 0 { format::won(r.spend_total) } else { "–".into() }} }
                    td { class: "r main-metric", "data-label": "ROAS",
                        {format::roas(r.roas)}
                        if compare {
                            ChangeTag { change: total_roas_change, class: "m-only" }
                        }
                    }
                    if compare {
                        td { class: "r cmp-chg m-hide", "data-label": "ROAS 변화", ChangeTag { change: total_roas_change } }
                    }
                } }
            }
        }
    }
}

#[component]
pub fn Revenue(pid: ReadSignal<i32>) -> Element {
    let ctx = use_app();
    let mut model = use_signal(|| None::<Model>);
    let mut lookback = use_signal(|| None::<u8>);
    // 쪽 번호는 (프로젝트, 기간, lookback)에, 열어 둔 주문은 프로젝트에 묶는다.
    // 프로젝트·기간을 바꾸면 같은 렌더에서 첫 쪽·닫힌 패널로 돌아간다.
    let mut offset = use_signal(|| None::<(OrdersKey, u32)>);
    let mut order = use_signal(|| None::<(i32, String)>);

    let settings = use_resource(move || {
        let p = pid();
        async move {
            api::get::<AttributionSettingsDto>(&format!("/v1/projects/{p}/attribution-settings"))
                .await
        }
    });
    let (cur_model, cur_lb, lookbacks) = pick(settings, model, lookback);
    let ready = settings.read().as_ref().is_some_and(|r| r.is_ok());

    let revenue = use_resource(move || {
        let (p, r, (m, lb, _)) = (pid(), ctx.range(), pick(settings, model, lookback));
        async move {
            let lb = lb.ok_or(crate::logic::errors::ApiError::NotFound)?;
            api::get::<RevenueReport>(&query::revenue(p, r, m, lb)).await
        }
    });
    // 이전 기간 (비교를 켰을 때만, 같은 모델·lookback)
    let prev_res = use_resource(move || {
        let (p, r, cmp, (m, lb, _)) = (
            pid(),
            ctx.range(),
            ctx.compare(),
            pick(settings, model, lookback),
        );
        async move {
            let Some(lb) = lb.filter(|_| cmp) else {
                return Ok(None);
            };
            api::get::<RevenueReport>(&query::revenue(p, r.previous(), m, lb))
                .await
                .map(Some)
        }
    });
    let prev = loaded_prev(&prev_res);
    let overlap = use_resource(move || {
        let (p, r, (_, lb, _)) = (pid(), ctx.range(), pick(settings, model, lookback));
        async move {
            let lb = lb.ok_or(crate::logic::errors::ApiError::NotFound)?;
            api::get::<OverlapReport>(&query::overlap(p, r, lb, 8)).await
        }
    });
    let orders = use_resource(move || {
        let (p, r, (_, lb, _)) = (pid(), ctx.range(), pick(settings, model, lookback));
        let off = scoped(&offset(), &(p, r, lb)).unwrap_or(0);
        async move {
            let lb = lb.ok_or(crate::logic::errors::ApiError::NotFound)?;
            api::get::<RecentOrdersReport>(&query::orders(p, r, lb, ORDERS_PAGE, off)).await
        }
    });
    let range = ctx.range();
    let orders_key: OrdersKey = (pid(), range, cur_lb);
    let cur_offset = scoped(&offset(), &orders_key).unwrap_or(0);
    let mut set_offset = move |v: u32| offset.set(Some((orders_key, v)));
    let cur_order = scoped(&order(), &pid());
    let mut open_order = move |id: String| order.set(Some((pid(), id)));

    rsx! {
        PageHead {
            title: "실제 매출",
            desc: format!("{} · 주문번호 기준으로 한 번만 센 매출을 매체별로 나눴어요", range.label()),
            actions: rsx! {
                if lookbacks.len() > 1 {
                    SegmentedControl {
                        options: lookbacks.iter().map(|d| (*d, format!("{d}일 이내 클릭"))).collect::<Vec<_>>(),
                        value: cur_lb.unwrap_or_default(),
                        onchange: move |v| lookback.set(Some(v)),
                    }
                } else if let Some(d) = cur_lb {
                    Badge { "{d}일 이내 클릭 기준" }
                }
            },
        }

        if !ready {
            {load(settings, rsx! { SkeletonCards { n: 3 } }, |_| rsx! {})}
        } else if needs_lookback_setup(ready, &lookbacks) {
            Card {
                EmptyState {
                    icon: crate::components::icon::IconName::Settings,
                    title: "클릭 인정 기간이 설정되지 않았어요",
                    desc: "실제 매출은 ‘며칠 이내 클릭’을 기준으로 나눠요. 기여 설정에서 기간(예: 7, 30)을 넣어 주세요.",
                    action: rsx! {
                        Link { class: "btn btn-primary", to: Route::Settings { pid: pid(), tab: "attribution".into() }, "기여 설정으로 이동" }
                    },
                }
            }
        } else {
            {load(revenue, rsx! { SkeletonCards { n: 3 } }, |r| {
                let pv = same_basis(&r, prev.clone());
                let pv = pv.as_ref();
                rsx! {
                    div { class: "grid grid-3",
                        StatCard {
                            label: "매체 합산 매출",
                            value: format::won_short(r.raw_sum),
                            change: pv.map(|p| change(r.raw_sum as f64, Some(p.raw_sum as f64))),
                            sub: match pv {
                                Some(p) => format!("이전 {} · 매체 리포트 단순 합산", format::won_short(p.raw_sum)),
                                None => format!("매체 리포트를 단순 합산 · ROAS {}", format::roas((r.spend_total > 0).then(|| r.raw_sum as f64 / r.spend_total as f64))),
                            },
                            style: StatStyle::Struck,
                        }
                        StatCard {
                            label: "실제 매출",
                            value: format::won_short(r.deduplicated),
                            change: pv.map(|p| change(r.deduplicated as f64, Some(p.deduplicated as f64))),
                            sub: match pv {
                                Some(p) => format!("이전 {} · 주문 {}건", format::won_short(p.deduplicated), format::count(r.orders)),
                                None => format!("주문 {}건 · ROAS {}", format::count(r.orders), format::roas(r.roas)),
                            },
                            style: StatStyle::Hero,
                        }
                        StatCard {
                            label: "매체 간 중복률",
                            value: format::pct(r.duplicate_rate, 1),
                            change: pv.map(|p| pp(r.duplicate_rate, Some(p.duplicate_rate))),
                            sub: match pv {
                                Some(p) => format!("이전 {} · {} 중복 제거", format::pct(p.duplicate_rate, 1), format::won_short(r.duplicate_amount)),
                                None => format!("{} 중복 제거", format::won_short(r.duplicate_amount)),
                            },
                        }
                    }
                }
            })}

            Card {
                title: "매체별 매출 기여",
                actions: rsx! {
                    SegmentedControl {
                        options: labels::MODELS.iter().map(|m| (*m, labels::model(*m).to_string())).collect::<Vec<_>>(),
                        value: cur_model,
                        onchange: move |m| model.set(Some(m)),
                    }
                },
                div { class: "callout", style: "margin-bottom: 20px;",
                    strong { "{labels::model(cur_model)}" }
                    " · {labels::model_help(cur_model)} 어떤 방식으로 나눠도 합계는 실제 매출과 같아요."
                }
                {load(revenue, rsx! { SkeletonBlock { rows: 5 } }, |r| {
                    if r.channels.is_empty() {
                        return rsx! { EmptyState { icon: crate::components::icon::IconName::Revenue, title: "기여 연산된 주문이 없어요", desc: "주문 서버 연동(S2S) 후 기여 연산이 끝나면 매체별 매출이 나타나요. 연산은 몇 분마다 돌아요." } };
                    }
                    let msg = explain(&r);
                    rsx! {
                        ShareBar { parts: r.channels.iter().map(|c| (labels::channel(&c.channel), c.attributed)).collect::<Vec<_>>(), money: true }
                        ChannelRevenueTable { report: r.clone(), prev: same_basis(&r, prev.clone()) }
                        if r.spend_total == 0 {
                            p { class: "hint", "광고비를 올리면 ROAS가 계산돼요 (설정 → 광고비)." }
                        }
                        if let Some(m) = msg {
                            ExplainCard { text: m }
                        }
                    }
                })}
            }

            div { class: "grid grid-2 align-start",
                Card { title: "중복이 생기는 매체 조합",
                    {load(overlap, rsx! { SkeletonBlock { rows: 5 } }, |o| rsx! {
                        if o.paths.is_empty() {
                            EmptyState { icon: crate::components::icon::IconName::Acquisition, title: "여러 매체를 거친 주문이 없어요", desc: "한 매체만 거쳐 구매한 주문뿐이라 중복이 없습니다." }
                        } else {
                            p { class: "page-desc", style: "margin: 0 0 16px",
                                "서로 다른 매체를 2개 이상 거친 주문 "
                                strong { "{format::count(o.multi_channel_orders)}건" }
                            }
                            BarList { items: o.paths.iter().map(|p| (path_label(&p.channels), p.orders as i64)).collect::<Vec<_>>() }
                        }
                    })}
                }
                Card { title: "최근 주문",
                    {load(orders, rsx! { SkeletonBlock { rows: 6 } }, |o| rsx! {
                        if o.rows.is_empty() && o.total == 0 {
                            EmptyState { icon: crate::components::icon::IconName::Revenue, title: "이 기간에 주문이 없어요", desc: "주문은 S2S 주문 API로 받은 것만 보여요." }
                        } else if o.rows.is_empty() {
                            div { class: "callout", "이 쪽에는 주문이 없어요. 이전 쪽으로 돌아가 보세요." }
                        } else {
                            div { class: "table-wrap",
                                table { class: "table table-cards",
                                    thead { tr {
                                        th { scope: "col", "주문" }
                                        th { scope: "col", "마지막 클릭" }
                                        th { class: "r", scope: "col", "금액" }
                                    } }
                                    tbody {
                                        for r in o.rows.iter() {
                                            tr {
                                                key: "{r.order_id}",
                                                class: "clickable",
                                                tabindex: "0",
                                                onclick: { let id = r.order_id.clone(); move |_| open_order(id.clone()) },
                                                onkeydown: {
                                                    let id = r.order_id.clone();
                                                    move |e: KeyboardEvent| if is_activation_key(&e.key()) {
                                                        e.prevent_default();
                                                        open_order(id.clone());
                                                    }
                                                },
                                                td { "data-label": "주문",
                                                    div { class: "cell-title mono", "{r.order_id}" }
                                                    div { class: "cell-sub", "{dates::datetime_kr(&r.ordered_at)}" }
                                                }
                                                td { "data-label": "마지막 클릭",
                                                    {
                                                        let (st, cancelled) = labels::order_status(&r.status);
                                                        rsx! {
                                                            if cancelled {
                                                                Badge { tone: Tone::Down, "{st}" }
                                                            } else if let Some(c) = &r.last_click_channel {
                                                                "{labels::channel(c)}"
                                                            } else {
                                                                span { class: "muted", "연산 전" }
                                                            }
                                                        }
                                                    }
                                                }
                                                td { class: "r", "data-label": "금액", "{format::won(r.effective_amount)}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        // 현재 쪽이 비어도 전체 건수가 있으면 쪽 넘김을 보여 돌아갈 수 있게 한다.
                        if let Some(pv) = pager(cur_offset, o.rows.len(), o.total, ORDERS_PAGE) {
                            div { class: "pager",
                                span { class: "muted", "{pv.label()}" }
                                Button { variant: Variant::Ghost, size: Size::Sm, disabled: !pv.has_prev, onclick: move |_| set_offset(cur_offset.saturating_sub(ORDERS_PAGE)), "이전" }
                                Button { variant: Variant::Ghost, size: Size::Sm, disabled: !pv.has_next, onclick: move |_| set_offset(cur_offset + ORDERS_PAGE), "다음" }
                            }
                        }
                    })}
                }
            }
        }

        if let (Some(id), Some(lb)) = (cur_order, cur_lb) {
            OrderDrawer { pid: pid(), order_id: id, lookback: lb, onclose: move |_| order.set(None) }
        }
    }
}

/// 최근 주문 쪽 번호를 묶는 맥락: (프로젝트, 기간, lookback)
type OrdersKey = (i32, dates::DateRange, Option<u8>);

#[component]
fn ExplainCard(text: String) -> Element {
    let mut ctx = use_app();
    rsx! {
        div { class: "explain",
            div { class: "spread",
                div { class: "gallery-label", "광고주에게 이렇게 설명하세요" }
                Button {
                    variant: Variant::Secondary,
                    size: Size::Sm,
                    onclick: {
                        let t = text.clone();
                        move |_| { copy_to_clipboard(&t); ctx.notify("문구를 복사했어요"); }
                    },
                    "문구 복사"
                }
            }
            p { class: "quote", "{text}" }
        }
    }
}

#[component]
fn OrderDrawer(
    pid: ReadSignal<i32>,
    order_id: ReadSignal<String>,
    lookback: ReadSignal<u8>,
    onclose: EventHandler<()>,
) -> Element {
    // 입력을 리소스 안에서 읽어, 다른 주문·lookback으로 바뀌면 다시 불러온다.
    let res = use_resource(move || {
        let (p, id, lb) = (pid(), order_id(), lookback());
        async move { api::get::<OrderDetail>(&query::order(p, &id, lb)).await }
    });
    rsx! {
        Drawer { open: true, title: "주문 {order_id}", onclose: move |_| onclose.call(()),
            {load(res, rsx! { SkeletonBlock { rows: 8 } }, |d| {
                let (st, cancelled) = labels::order_status(&d.order.status);
                rsx! {
                    div { class: "order-summary",
                        div { class: "stat-label", "실제 매출로 집계된 금액" }
                        div { class: "stat-value", "{format::won(d.order.effective_amount)}" }
                        div { class: "row",
                            Badge { tone: if cancelled { Tone::Down } else { Tone::Up }, "{st}" }
                            span { class: "muted", "{dates::datetime_kr(&d.order.ordered_at)} · {d.order.source}" }
                        }
                        if d.order.refunded_amount > 0 {
                            div { class: "muted", "결제 {format::won(d.order.net_amount)} − 환불 {format::won(d.order.refunded_amount)}" }
                        }
                    }
                    div {
                        div { class: "section-label", "구매까지 거친 클릭 ({d.lookback_days}일 이내)" }
                        if d.touches.is_empty() {
                            div { class: "callout", "기간 안에 광고 클릭이 없어요. 직접 방문 또는 미귀속 주문입니다." }
                        } else {
                            div { class: "timeline",
                                for (i, t) in d.touches.iter().enumerate() {
                                    div { key: "{t.touch_index}", class: "tl-item",
                                        div { class: if i + 1 == d.touches.len() { "tl-dot primary" } else { "tl-dot" } }
                                        div {
                                            div { class: "tl-time", "{dates::datetime_kr(&t.ts)}" }
                                            div { class: "tl-title", "{labels::channel(&t.channel)}" }
                                            if !t.campaign.is_empty() {
                                                div { class: "cell-sub", "캠페인 {t.campaign}" }
                                            }
                                        }
                                    }
                                }
                                div { class: "tl-item",
                                    div { class: "tl-dot primary" }
                                    div {
                                        div { class: "tl-time", "{dates::datetime_kr(&d.order.ordered_at)}" }
                                        div { class: "tl-title", "구매 {format::won(d.order.effective_amount)}" }
                                    }
                                }
                            }
                        }
                    }
                    div {
                        div { class: "section-label", "기여 방식별 배분" }
                        if d.shares.is_empty() {
                            div { class: "callout", "아직 기여 연산 전이에요. 몇 분 뒤 다시 열어 보세요." }
                        }
                        for s in d.shares.iter().filter(|s| s.model.conserves_revenue()) {
                            div { key: "{s.model.as_str()}", class: "share-block",
                                div { class: "share-model", "{labels::model(s.model)}" }
                                for (k, row) in s.rows.iter().enumerate() {
                                    div { key: "{k}", class: "share-row",
                                        span {
                                            if row.touch_index == 0 { "미귀속" } else { "{row.touch_index}. {labels::channel(&row.channel)}" }
                                            if row.is_assist {
                                                span { class: "badge badge-neutral", style: "margin-left: 6px", "어시스트" }
                                            }
                                        }
                                        span { class: "num", "{format::won(row.amount)}" }
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

#[cfg(test)]
mod tests {
    use super::*;
    use analytics_core::dto::reports::ReportRange;

    fn report(raw: i64, dedup: i64, spend: i64) -> RevenueReport {
        RevenueReport {
            range: ReportRange {
                from: "2026-11-01".into(),
                to: "2026-11-07".into(),
            },
            model: Model::LastClick,
            lookback_days: 7,
            raw_sum: raw,
            deduplicated: dedup,
            duplicate_amount: (raw - dedup).max(0),
            duplicate_rate: if raw > 0 {
                (raw - dedup).max(0) as f64 / raw as f64
            } else {
                0.0
            },
            orders: 3,
            spend_total: spend,
            roas: (spend > 0).then(|| dedup as f64 / spend as f64),
            channels: vec![],
        }
    }

    #[test]
    fn explanation_uses_real_numbers() {
        let m = explain(&report(500, 325, 100)).unwrap();
        assert!(m.contains("ROAS 500%가 325%로"), "{m}");
        assert!(m.contains("35%"));
        let m = explain(&report(200_000_000, 130_000_000, 0)).unwrap();
        assert!(
            m.contains("2억 원이 실제 매출 1.3억 원로") || m.contains("2억 원"),
            "{m}"
        );
        assert_eq!(explain(&report(100, 100, 0)), None);
        assert_eq!(explain(&report(0, 0, 0)), None);
    }

    #[test]
    fn empty_lookbacks_point_to_settings() {
        assert!(needs_lookback_setup(true, &[]));
        assert!(!needs_lookback_setup(true, &[7]));
        // 아직 설정을 못 읽었으면 로딩·오류 상태를 보여준다
        assert!(!needs_lookback_setup(false, &[]));
    }

    #[test]
    fn overlap_path_labels() {
        assert_eq!(
            path_label(&["naver_sa".into(), "meta".into()]),
            "네이버 검색광고 → 메타"
        );
        assert_eq!(path_label(&[]), "");
    }
}

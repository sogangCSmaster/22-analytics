//! 실제 매출(중복 소거): 매체 합산 vs 실제 매출 vs 중복률, 모델·lookback 선택, 매체별 기여·ROAS,
//! 매체 조합 경로, 최근 주문 → 주문 상세(접점 타임라인·모델별 배분) 패널.

use crate::api;
use crate::components::chart::{BarList, ShareBar};
use crate::components::ui::{
    Badge, Button, Card, Drawer, EmptyState, PageHead, SegmentedControl, Size, SkeletonBlock,
    SkeletonCards, StatCard, StatStyle, Tone, Variant, copy_to_clipboard, load,
};
use crate::format;
use crate::logic::{dates, labels, query};
use crate::state::use_app;
use analytics_core::attribution::Model;
use analytics_core::dto::admin::AttributionSettingsDto;
use analytics_core::dto::reports::{OrderDetail, OverlapReport, RecentOrdersReport, RevenueReport};
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

#[component]
pub fn Revenue(pid: ReadSignal<i32>) -> Element {
    let ctx = use_app();
    let mut model = use_signal(|| None::<Model>);
    let mut lookback = use_signal(|| None::<u8>);
    let mut offset = use_signal(|| 0u32);
    let mut order = use_signal(|| None::<String>);

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
    let overlap = use_resource(move || {
        let (p, r, (_, lb, _)) = (pid(), ctx.range(), pick(settings, model, lookback));
        async move {
            let lb = lb.ok_or(crate::logic::errors::ApiError::NotFound)?;
            api::get::<OverlapReport>(&query::overlap(p, r, lb, 8)).await
        }
    });
    let orders = use_resource(move || {
        let (p, r, (_, lb, _), off) = (
            pid(),
            ctx.range(),
            pick(settings, model, lookback),
            offset(),
        );
        async move {
            let lb = lb.ok_or(crate::logic::errors::ApiError::NotFound)?;
            api::get::<RecentOrdersReport>(&query::orders(p, r, lb, ORDERS_PAGE, off)).await
        }
    });
    let range = ctx.range();

    rsx! {
        PageHead {
            title: "실제 매출",
            desc: format!("{} · 주문번호 기준으로 한 번만 센 매출을 매체별로 나눴어요", range.label()),
            actions: rsx! {
                if lookbacks.len() > 1 {
                    SegmentedControl {
                        options: lookbacks.iter().map(|d| (*d, format!("{d}일 이내 클릭"))).collect::<Vec<_>>(),
                        value: cur_lb.unwrap_or_default(),
                        onchange: move |v| { lookback.set(Some(v)); offset.set(0); },
                    }
                } else if let Some(d) = cur_lb {
                    Badge { "{d}일 이내 클릭 기준" }
                }
            },
        }

        if !ready {
            {load(settings, rsx! { SkeletonCards { n: 3 } }, |_| rsx! {})}
        } else {
            {load(revenue, rsx! { SkeletonCards { n: 3 } }, |r| rsx! {
                div { class: "grid grid-3",
                    StatCard {
                        label: "매체 합산 매출",
                        value: format::won_short(r.raw_sum),
                        sub: format!("매체 리포트를 단순 합산 · ROAS {}", format::roas((r.spend_total > 0).then(|| r.raw_sum as f64 / r.spend_total as f64))),
                        style: StatStyle::Struck,
                    }
                    StatCard {
                        label: "실제 매출",
                        value: format::won_short(r.deduplicated),
                        sub: format!("주문 {}건 · ROAS {}", format::count(r.orders), format::roas(r.roas)),
                        style: StatStyle::Hero,
                    }
                    StatCard {
                        label: "매체 간 중복률",
                        value: format::pct(r.duplicate_rate, 1),
                        sub: format!("{} 중복 제거", format::won_short(r.duplicate_amount)),
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
                        return rsx! { EmptyState { icon: "₩", title: "기여 연산된 주문이 없어요", desc: "주문 서버 연동(S2S) 후 기여 연산이 끝나면 매체별 매출이 나타나요. 연산은 몇 분마다 돌아요." } };
                    }
                    let msg = explain(&r);
                    rsx! {
                        ShareBar { parts: r.channels.iter().map(|c| (labels::channel(&c.channel), c.attributed)).collect::<Vec<_>>(), money: true }
                        div { class: "table-wrap", style: "margin-top: 16px;",
                            table { class: "table",
                                thead { tr {
                                    th { scope: "col", "매체" }
                                    th { class: "r", scope: "col", "매체가 주장한 매출" }
                                    th { class: "r", scope: "col", "기여 매출" }
                                    th { class: "r", scope: "col", "비중" }
                                    th { class: "r", scope: "col", "어시스트" }
                                    th { class: "r", scope: "col", "광고비" }
                                    th { class: "r", scope: "col", "ROAS" }
                                } }
                                tbody {
                                    for c in r.channels.iter() {
                                        tr { key: "{c.channel}",
                                            td { div { class: "cell-title", "{labels::channel(&c.channel)}" } }
                                            td { class: "r muted strike-soft", "{format::won(c.raw_claimed)}" }
                                            td { class: "r strong", "{format::won(c.attributed)}" }
                                            td { class: "r", {format::pct(c.share, 1)} }
                                            td { class: "r", "{format::count(c.assists)}" }
                                            td { class: "r", {if c.spend > 0 { format::won(c.spend) } else { "–".into() }} }
                                            td { class: "r", {format::roas(c.roas)} }
                                        }
                                    }
                                }
                                tfoot { tr {
                                    td { "합계" }
                                    td { class: "r", "{format::won(r.raw_sum)}" }
                                    td { class: "r", "{format::won(r.deduplicated)}" }
                                    td { class: "r", "100%" }
                                    td { class: "r", "" }
                                    td { class: "r", {if r.spend_total > 0 { format::won(r.spend_total) } else { "–".into() }} }
                                    td { class: "r", {format::roas(r.roas)} }
                                } }
                            }
                        }
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
                            EmptyState { icon: "⇄", title: "여러 매체를 거친 주문이 없어요", desc: "한 매체만 거쳐 구매한 주문뿐이라 중복이 없습니다." }
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
                        if o.rows.is_empty() {
                            EmptyState { icon: "🧾", title: "이 기간에 주문이 없어요", desc: "주문은 S2S 주문 API로 받은 것만 보여요." }
                        } else {
                            div { class: "table-wrap",
                                table { class: "table",
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
                                                onclick: { let id = r.order_id.clone(); move |_| order.set(Some(id.clone())) },
                                                onkeydown: { let id = r.order_id.clone(); move |e: KeyboardEvent| if e.key() == Key::Enter { order.set(Some(id.clone())) } },
                                                td {
                                                    div { class: "cell-title mono", "{r.order_id}" }
                                                    div { class: "cell-sub", "{dates::datetime_kr(&r.ordered_at)}" }
                                                }
                                                td {
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
                                                td { class: "r", "{format::won(r.effective_amount)}" }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "pager",
                                span { class: "muted", "{offset() + 1}–{offset() as u64 + o.rows.len() as u64} / {format::count(o.total)}건" }
                                Button { variant: Variant::Ghost, size: Size::Sm, disabled: offset() == 0, onclick: move |_| offset.set(offset().saturating_sub(ORDERS_PAGE)), "이전" }
                                Button { variant: Variant::Ghost, size: Size::Sm, disabled: (offset() as u64 + ORDERS_PAGE as u64) >= o.total, onclick: move |_| offset.set(offset() + ORDERS_PAGE), "다음" }
                            }
                        }
                    })}
                }
            }
        }

        if let (Some(id), Some(lb)) = (order(), cur_lb) {
            OrderDrawer { pid: pid(), order_id: id, lookback: lb, onclose: move |_| order.set(None) }
        }
    }
}

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
fn OrderDrawer(pid: i32, order_id: String, lookback: u8, onclose: EventHandler<()>) -> Element {
    let id = order_id.clone();
    let res = use_resource(move || {
        let id = id.clone();
        async move { api::get::<OrderDetail>(&query::order(pid, &id, lookback)).await }
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
    fn overlap_path_labels() {
        assert_eq!(
            path_label(&["naver_sa".into(), "meta".into()]),
            "네이버 검색광고 → 메타"
        );
        assert_eq!(path_label(&[]), "");
    }
}

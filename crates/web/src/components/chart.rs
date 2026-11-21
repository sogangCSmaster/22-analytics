//! 가벼운 SVG·HTML 차트 (외부 차트 라이브러리 없이 wasm 크기를 작게 유지).
//!
//! 좌표 계산은 [`crate::logic::chart_math`]. 마우스를 올리면 툴팁을 보여주는 감지 영역은 HTML로
//! 겹쳐 놓아 SVG 좌표 변환 없이 동작한다. 모든 라벨은 텍스트 노드로만 그린다.

use crate::format;
use crate::logic::chart_math::{
    Frame, area_path, axis_label, bars, hover_zones, label_step, line_path, ratio_pct, shares,
    ticks, tooltip_anchor,
};
use dioxus::prelude::*;

/// 범주 색 개수. 실제 색은 CSS `.c0`~`.c5` (tokens.css의 --chart-1~6, primary 계열 중심).
/// 인라인 style로 넣으면 갱신 때 브라우저가 var() 배경을 떨어뜨리는 경우가 있어 클래스로 준다.
pub const PALETTE_N: usize = 6;

#[derive(Clone, PartialEq, Debug)]
pub struct Series {
    pub name: String,
    pub values: Vec<f64>,
    /// 비교(이전 기간) 계열: 회색 점선
    pub muted: bool,
}

fn fmt_value(v: f64, money: bool) -> String {
    if money {
        format::won(v.round() as i64)
    } else if v.fract() != 0.0 {
        format::trim_float(v, 2)
    } else {
        format::int(v as i64)
    }
}

/// 추이 선 차트. 첫 계열이 주 계열, 나머지는 비교 계열(점선).
#[component]
pub fn LineChart(
    series: Vec<Series>,
    labels: Vec<String>,
    /// 툴팁 제목 (라벨과 다를 때, 예: 날짜 전체 표기)
    tooltip_titles: Option<Vec<String>>,
    #[props(default = 240)] height: u32,
    #[props(default)] money: bool,
    #[props(default = "추이 차트".to_string())] aria_label: String,
) -> Element {
    let mut hover = use_signal(|| None::<usize>);
    let f = Frame {
        w: 800.0,
        h: height as f64,
        pad_l: 52.0,
        pad_r: 12.0,
        pad_t: 12.0,
        pad_b: 30.0,
    };
    let n = series.first().map(|s| s.values.len()).unwrap_or(0);
    let max = series
        .iter()
        .flat_map(|s| s.values.iter().copied())
        .filter(|v| v.is_finite())
        .fold(0.0f64, f64::max);
    // 작은 수에서 0.25 같은 소수 눈금이 생기지 않게
    let grid = ticks(max.max(4.0), 4);
    let top = *grid.last().unwrap_or(&1.0);
    let step = label_step(n, 7);
    let zones = hover_zones(&f, n);
    let titles = tooltip_titles.unwrap_or_else(|| labels.clone());
    let main_values = series.first().map(|s| s.values.clone()).unwrap_or_default();
    let area = area_path(&main_values, &f, top);
    let px = move |x: f64| x / f.w * 100.0;
    let hovered = hover().filter(|i| *i < n);

    rsx! {
        div { class: "chart line-chart",
            div { class: "plot", style: "height: {height}px",
                svg {
                    view_box: "0 0 {f.w} {f.h}",
                    role: "img",
                    "aria-label": "{aria_label}",
                    preserve_aspect_ratio: "none",
                    defs {
                        linearGradient { id: "area-fill", x1: "0", y1: "0", x2: "0", y2: "1",
                            stop { offset: "0%", stop_color: "var(--chart-1)", stop_opacity: "0.16" }
                            stop { offset: "100%", stop_color: "var(--chart-1)", stop_opacity: "0" }
                        }
                    }
                    for (k, t) in grid.iter().enumerate() {
                        line { key: "g{k}", x1: "{f.pad_l}", x2: "{f.w - f.pad_r}", y1: "{f.y(*t, top)}", y2: "{f.y(*t, top)}", stroke: "var(--line)", stroke_width: "1", vector_effect: "non-scaling-stroke" }
                    }
                    if n > 0 {
                        path { d: "{area}", fill: "url(#area-fill)" }
                    }
                    for (si, s) in series.iter().enumerate().rev() {
                        path {
                            key: "s{si}",
                            d: "{line_path(&s.values, &f, top)}",
                            fill: "none",
                            stroke: if s.muted { "var(--text-disabled)" } else { "var(--chart-1)" },
                            stroke_width: if s.muted { "2" } else { "2.5" },
                            stroke_dasharray: if s.muted { "5 5" } else { "none" },
                            stroke_linejoin: "round",
                            stroke_linecap: "round",
                            vector_effect: "non-scaling-stroke",
                        }
                    }
                    if let Some(i) = hovered {
                        line { x1: "{f.x(i, n)}", x2: "{f.x(i, n)}", y1: "{f.pad_t}", y2: "{f.bottom()}", stroke: "var(--text-weak)", stroke_width: "1", stroke_dasharray: "3 3", vector_effect: "non-scaling-stroke" }
                    }
                }
                div { class: "y-axis",
                    for (k, t) in grid.iter().enumerate() {
                        span { key: "y{k}", style: "top: {f.y(*t, top) / f.h * 100.0:.2}%", "{axis_label(*t)}" }
                    }
                }
                div { class: "x-axis",
                    for (i, l) in labels.iter().enumerate().filter(|(i, _)| i % step == 0) {
                        span { key: "x{i}", style: "left: {px(f.x(i, n)):.2}%", "{l}" }
                    }
                }
                if let Some(i) = hovered {
                    for (si, s, v) in series.iter().enumerate().filter_map(|(si, s)| s.values.get(i).map(|v| (si, s, v))) {
                            span {
                                key: "d{si}",
                                class: if s.muted { "dot muted" } else { "dot" },
                                style: "left: {px(f.x(i, n)):.2}%; top: {f.y(*v, top) / f.h * 100.0:.2}%",
                            }
                    }
                }
                div { class: "hover-layer",
                    for (i, (left, width)) in zones.iter().enumerate() {
                        div {
                            key: "z{i}",
                            class: "hover-zone",
                            style: "left: {left:.3}%; width: {width:.3}%",
                            onmouseenter: move |_| hover.set(Some(i)),
                            onmouseleave: move |_| hover.set(None),
                            onclick: move |_| hover.set(Some(i)),
                        }
                    }
                }
                if let Some(i) = hovered {
                    Tooltip {
                        x_pct: px(f.x(i, n)),
                        title: titles.get(i).cloned().unwrap_or_default(),
                        rows: series
                            .iter()
                            .map(|s| (s.name.clone(), s.values.get(i).map(|v| fmt_value(*v, money)).unwrap_or_else(|| "–".into()), s.muted))
                            .collect::<Vec<_>>(),
                    }
                }
            }
            if series.len() > 1 {
                div { class: "legend chart-legend",
                    for (si, s) in series.iter().enumerate() {
                        span { key: "l{si}",
                            i { class: if s.muted { "swatch dashed" } else { "swatch" } }
                            "{s.name}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn Tooltip(x_pct: f64, title: String, rows: Vec<(String, String, bool)>) -> Element {
    let anchor = tooltip_anchor(x_pct);
    rsx! {
        div { class: "tooltip {anchor}", role: "status", style: "left: {x_pct:.2}%",
            div { class: "tt-title", "{title}" }
            for (k, (name, value, muted)) in rows.into_iter().enumerate() {
                div { key: "{k}", class: "tt-row",
                    i { class: if muted { "swatch dashed" } else { "swatch" } }
                    span { class: "tt-name", "{name}" }
                    span { class: "tt-value", "{value}" }
                }
            }
        }
    }
}

/// 세로 막대 차트 (분당 페이지뷰 등). 마지막 막대를 강조할 수 있다.
#[component]
pub fn ColumnChart(
    values: Vec<f64>,
    labels: Vec<String>,
    #[props(default = 180)] height: u32,
    #[props(default)] highlight_last: bool,
    #[props(default = "값".to_string())] value_name: String,
    #[props(default = "막대 차트".to_string())] aria_label: String,
) -> Element {
    let mut hover = use_signal(|| None::<usize>);
    let f = Frame {
        w: 800.0,
        h: height as f64,
        pad_l: 40.0,
        pad_r: 4.0,
        pad_t: 8.0,
        pad_b: 26.0,
    };
    let n = values.len();
    let max = values
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0f64, f64::max);
    let grid = ticks(max.max(3.0), 3);
    let top = *grid.last().unwrap_or(&1.0);
    let geo = bars(&values, &f, top, 0.3);
    let step = label_step(n, 6);
    let center = |i: usize| (geo[i].0 + geo[i].2 / 2.0) / f.w * 100.0;
    // 막대 가운데를 점으로 보는 프레임 (마우스 감지 구간 계산용)
    let half_slot = if n > 0 {
        f.plot_w() / n as f64 / 2.0
    } else {
        0.0
    };
    let centers = Frame {
        pad_l: f.pad_l + half_slot,
        pad_r: f.pad_r + half_slot,
        ..f
    };
    let hovered = hover().filter(|i| *i < n);
    rsx! {
        div { class: "chart column-chart",
            div { class: "plot", style: "height: {height}px",
                svg { view_box: "0 0 {f.w} {f.h}", role: "img", "aria-label": "{aria_label}", preserve_aspect_ratio: "none",
                    for (k, t) in grid.iter().enumerate() {
                        line { key: "g{k}", x1: "{f.pad_l}", x2: "{f.w - f.pad_r}", y1: "{f.y(*t, top)}", y2: "{f.y(*t, top)}", stroke: "var(--line)", stroke_width: "1", vector_effect: "non-scaling-stroke" }
                    }
                    for (i, (x, y, w, h)) in geo.iter().enumerate() {
                        rect {
                            key: "b{i}",
                            x: "{x}", y: "{y}", width: "{w}", height: "{h.max(0.0)}", rx: "2",
                            fill: if hovered == Some(i) || (highlight_last && i + 1 == n) { "var(--chart-1)" } else { "var(--chart-2)" },
                        }
                    }
                }
                div { class: "y-axis",
                    for (k, t) in grid.iter().enumerate() {
                        span { key: "y{k}", style: "top: {f.y(*t, top) / f.h * 100.0:.2}%", "{axis_label(*t)}" }
                    }
                }
                div { class: "x-axis",
                    for (i, l) in labels.iter().enumerate().filter(|(i, _)| i % step == 0) {
                        span { key: "x{i}", style: "left: {center(i):.2}%", "{l}" }
                    }
                }
                div { class: "hover-layer",
                    for (i, (left, width)) in hover_zones(&centers, n).into_iter().enumerate() {
                        div {
                            key: "z{i}",
                            class: "hover-zone",
                            style: "left: {left:.3}%; width: {width:.3}%",
                            onmouseenter: move |_| hover.set(Some(i)),
                            onmouseleave: move |_| hover.set(None),
                            onclick: move |_| hover.set(Some(i)),
                        }
                    }
                }
                if let Some(i) = hovered {
                    Tooltip {
                        x_pct: center(i),
                        title: labels.get(i).cloned().unwrap_or_default(),
                        rows: vec![(value_name.clone(), fmt_value(values[i], false), false)],
                    }
                }
            }
        }
    }
}

/// 가로 막대 목록 (라벨 · 막대 · 값 · 비중).
#[component]
pub fn BarList(
    items: Vec<(String, i64)>,
    #[props(default)] money: bool,
    /// 오른쪽에 전체 대비 비중(%) 표시
    #[props(default)]
    show_share: bool,
    onselect: Option<EventHandler<usize>>,
) -> Element {
    let max = items.iter().map(|(_, v)| *v).max().unwrap_or(0) as f64;
    let total: i64 = items.iter().map(|(_, v)| (*v).max(0)).sum();
    let clickable = onselect.is_some();
    rsx! {
        div { class: "barlist", role: "list",
            for (i, (label, v)) in items.into_iter().enumerate() {
                div {
                    class: if clickable { "barlist-row clickable" } else { "barlist-row" },
                    key: "{i}",
                    role: "listitem",
                    title: "{label}",
                    onclick: move |_| {
                        if let Some(h) = onselect {
                            h.call(i)
                        }
                    },
                    div { class: "barlist-label", "{label}" }
                    div { class: "barlist-track",
                        div { class: "barlist-fill", style: "width: {ratio_pct(v as f64, max):.1}%" }
                    }
                    div { class: "barlist-value",
                        if money { {format::won_short(v)} } else { {format::int(v)} }
                        if show_share {
                            span { class: "barlist-share", {format::pct(if total > 0 { v as f64 / total as f64 } else { 0.0 }, 1)} }
                        }
                    }
                }
            }
        }
    }
}

/// 비율 막대: 하나의 합계를 여러 조각으로 나눠 보여준다 (채널 구성 등).
#[component]
pub fn ShareBar(parts: Vec<(String, i64)>, #[props(default)] money: bool) -> Element {
    let mut hover = use_signal(|| None::<usize>);
    let pcts = shares(&parts.iter().map(|p| p.1).collect::<Vec<_>>());
    let fmt = move |v: i64| {
        if money {
            format::won_short(v)
        } else {
            format::int(v)
        }
    };
    rsx! {
        div { class: "share-wrap",
            div { class: "split-bar", role: "img", aria_label: "구성 비율",
                for (i, ((label, v), p)) in parts.iter().zip(pcts.iter()).enumerate() {
                    div {
                            key: "{i}",
                            class: if hover() == Some(i) { "seg c{i % PALETTE_N} on" } else { "seg c{i % PALETTE_N}" },
                            title: "{label} {fmt(*v)} ({format::trim_float(*p, 1)}%)",
                            style: "flex: {p} 1 0",
                            hidden: *p <= 0.0,
                            onmouseenter: move |_| hover.set(Some(i)),
                            onmouseleave: move |_| hover.set(None),
                    }
                }
            }
            div { class: "legend", style: "margin-top: 12px;",
                for (i, ((label, v), p)) in parts.iter().zip(pcts.iter()).enumerate() {
                    span {
                        key: "{i}",
                        class: if hover() == Some(i) { "on" } else { "" },
                        onmouseenter: move |_| hover.set(Some(i)),
                        onmouseleave: move |_| hover.set(None),
                        i { class: "c{i % PALETTE_N}" }
                        "{label} "
                        b { "{format::trim_float(*p, 1)}%" }
                        if hover() == Some(i) {
                            span { class: "muted", " · {fmt(*v)}" }
                        }
                    }
                }
            }
        }
    }
}

/// 전자상거래 퍼널: 단계별 방문자 막대와 단계 사이 전환·이탈.
/// steps: (이름, 방문자, 이전 단계 대비, 첫 단계 대비)
#[component]
pub fn FunnelChart(steps: Vec<(String, u64, f64, f64)>) -> Element {
    let first = steps.first().map(|s| s.1).unwrap_or(0) as f64;
    rsx! {
        div { class: "funnel", role: "list",
            for (i, (label, visitors, rate_prev, rate_first)) in steps.into_iter().enumerate() {
                Fragment { key: "{i}",
                if i > 0 {
                    div { class: "funnel-drop",
                        span { class: "arrow", "↓" }
                        "다음 단계로 {format::pct(rate_prev, 1)}"
                        span { class: "muted", " · {format::pct((1.0 - rate_prev).max(0.0), 1)} 이탈" }
                    }
                }
                div { class: "funnel-step", role: "listitem",
                    div { class: "funnel-head",
                        span { class: "funnel-idx", "{i + 1}" }
                        span { class: "funnel-label", "{label}" }
                        span { class: "funnel-value", "{format::count(visitors)}명" }
                    }
                    div { class: "funnel-track",
                        div { class: "funnel-fill", style: "width: {funnel_width(visitors, first):.1}%" }
                    }
                    div { class: "funnel-rate", "첫 단계 대비 {format::pct(rate_first, 1)}" }
                }
                }
            }
        }
    }
}

fn funnel_width(v: u64, first: f64) -> f64 {
    let w = ratio_pct(v as f64, first);
    if v > 0 { w.max(0.8) } else { w }
}

/// 지표 카드 안의 작은 추이선.
#[component]
pub fn Sparkline(values: Vec<f64>) -> Element {
    let f = Frame {
        w: 80.0,
        h: 28.0,
        pad_l: 2.0,
        pad_r: 2.0,
        pad_t: 3.0,
        pad_b: 3.0,
    };
    let max = values
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0f64, f64::max);
    let top = if max > 0.0 { max } else { 1.0 };
    rsx! {
        svg { class: "sparkline", view_box: "0 0 80 28", width: "80", height: "28", "aria-hidden": "true",
            path { d: "{line_path(&values, &f, top)}", fill: "none", stroke: "var(--chart-1)", stroke_width: "1.75", stroke_linejoin: "round", stroke_linecap: "round" }
        }
    }
}

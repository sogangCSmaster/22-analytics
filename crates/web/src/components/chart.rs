//! 가벼운 SVG 차트. Phase 1에서 상호작용이 필요한 차트는 ECharts 래퍼로 바꾼다.

use crate::format;
use dioxus::prelude::*;

/// 면적 추이 차트.
#[component]
pub fn TrendChart(
    values: Vec<i64>,
    labels: Vec<String>,
    #[props(default = 220)] height: u32,
) -> Element {
    const W: f64 = 800.0;
    let h = height as f64;
    let (pad_l, pad_r, pad_t, pad_b) = (48.0, 8.0, 12.0, 28.0);
    let max = values.iter().copied().max().unwrap_or(1).max(1) as f64;
    let nice_max = nice_ceil(max);
    let n = values.len().max(2);
    let x = |i: usize| pad_l + (W - pad_l - pad_r) * i as f64 / (n - 1) as f64;
    let y = |v: i64| pad_t + (h - pad_t - pad_b) * (1.0 - v as f64 / nice_max);

    let line: String = values
        .iter()
        .enumerate()
        .map(|(i, v)| format!("{}{:.1},{:.1}", if i == 0 { "M" } else { "L" }, x(i), y(*v)))
        .collect();
    let area = format!(
        "{line}L{:.1},{:.1}L{:.1},{:.1}Z",
        x(values.len().saturating_sub(1)),
        h - pad_b,
        x(0),
        h - pad_b
    );
    let grid: Vec<(f64, String)> = (0..=4)
        .map(|k| {
            (
                y((nice_max * k as f64 / 4.0) as i64),
                format::int((nice_max * k as f64 / 4.0) as i64),
            )
        })
        .collect();
    let step = (labels.len() / 7).max(1);

    rsx! {
        div { class: "chart",
            svg { view_box: "0 0 {W} {h}", role: "img",
                defs {
                    linearGradient { id: "area-fill", x1: "0", y1: "0", x2: "0", y2: "1",
                        stop { offset: "0%", stop_color: "var(--chart-1)", stop_opacity: "0.18" }
                        stop { offset: "100%", stop_color: "var(--chart-1)", stop_opacity: "0" }
                    }
                }
                for (gy, label) in grid {
                    line { x1: "{pad_l}", x2: "{W - pad_r}", y1: "{gy}", y2: "{gy}", stroke: "var(--line)", stroke_width: "1" }
                    text { class: "axis", x: "{pad_l - 8.0}", y: "{gy + 4.0}", text_anchor: "end", "{label}" }
                }
                path { d: "{area}", fill: "url(#area-fill)" }
                path { d: "{line}", fill: "none", stroke: "var(--chart-1)", stroke_width: "2.5", stroke_linejoin: "round", stroke_linecap: "round" }
                for (i, label) in labels.iter().enumerate().filter(|(i, _)| i % step == 0) {
                    text { class: "axis", x: "{x(i)}", y: "{h - 6.0}", text_anchor: "middle", "{label}" }
                }
            }
        }
    }
}

fn nice_ceil(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let mag = 10f64.powf(v.log10().floor());
    let n = v / mag;
    let nice = if n <= 1.0 {
        1.0
    } else if n <= 2.0 {
        2.0
    } else if n <= 2.5 {
        2.5
    } else if n <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// 가로 막대 목록 (라벨 · 막대 · 값).
#[component]
pub fn BarList(items: Vec<(String, i64)>, #[props(default)] money: bool) -> Element {
    let max = items.iter().map(|(_, v)| *v).max().unwrap_or(1).max(1);
    rsx! {
        div { class: "barlist",
            for (label, v) in items {
                div { class: "barlist-row", key: "{label}",
                    div { class: "barlist-label", title: "{label}", "{label}" }
                    div { class: "barlist-track",
                        div { class: "barlist-fill", style: "width: {v as f64 / max as f64 * 100.0:.1}%" }
                    }
                    div { class: "barlist-value",
                        if money { {format::won_short(v)} } else { {format::int(v)} }
                    }
                }
            }
        }
    }
}

/// 비율 막대: 하나의 합계를 여러 조각으로 나눠 보여준다.
#[component]
pub fn SplitBar(parts: Vec<(String, i64, String)>) -> Element {
    let total: i64 = parts.iter().map(|(_, v, _)| *v).sum::<i64>().max(1);
    rsx! {
        div { class: "split-bar",
            for (label, v, color) in parts.iter().cloned() {
                div { key: "{label}", title: "{label}", style: "flex: {v as f64 / total as f64}; background: {color};" }
            }
        }
        div { class: "legend", style: "margin-top: 12px;",
            for (label, v, color) in parts {
                span { key: "{label}",
                    i { style: "background: {color};" }
                    "{label} {format::won_short(v)}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::nice_ceil;

    #[test]
    fn nice_ceiling() {
        assert_eq!(nice_ceil(12_830.0), 20_000.0);
        assert_eq!(nice_ceil(2_400.0), 2_500.0);
        assert_eq!(nice_ceil(0.0), 1.0);
    }
}

pub mod chart;
pub mod shell;
pub mod ui;

/// 적대적 데이터(이벤트 이름·경로·캠페인에 HTML/스크립트)가 텍스트로만 그려지는지 확인한다.
#[cfg(test)]
mod render_tests {
    use super::chart::{BarList, FunnelChart, LineChart, Series, ShareBar};
    use super::ui::{EmptyState, StatCard};
    use dioxus::prelude::*;

    const EVIL: &str = "<script>alert('x')</script><img src=x onerror=alert(1)>";

    fn assert_escaped(html: &str) {
        assert!(!html.contains("<script>"), "raw script tag leaked: {html}");
        assert!(!html.contains("<img"), "raw img tag leaked: {html}");
        assert!(html.contains("&#60;script&#62;"), "escaped text missing: {html}");
    }

    #[test]
    fn barlist_and_sharebar_escape_labels() {
        let html = dioxus_ssr::render_element(rsx! {
            BarList { items: vec![(EVIL.to_string(), 10), ("/상품?id=1&x=<b>".to_string(), 3)], show_share: true }
            ShareBar { parts: vec![(EVIL.to_string(), 5), ("메타".to_string(), 5)] }
        });
        assert_escaped(&html);
        assert!(html.contains("x=&#60;b&#62;"));
    }

    #[test]
    fn charts_escape_axis_labels_and_series_names() {
        let html = dioxus_ssr::render_element(rsx! {
            LineChart {
                series: vec![
                    Series { name: EVIL.into(), values: vec![1.0, 2.0], muted: false },
                    Series { name: "이전".into(), values: vec![0.0, 1.0], muted: true },
                ],
                labels: vec![EVIL.into(), "11.21".into()],
            }
            FunnelChart { steps: vec![(EVIL.to_string(), 10, 1.0, 1.0), ("구매".to_string(), 0, 0.0, 0.0)] }
        });
        assert_escaped(&html);
    }

    #[test]
    fn empty_and_edge_data_render() {
        let html = dioxus_ssr::render_element(rsx! {
            LineChart { series: vec![], labels: vec![] }
            LineChart { series: vec![Series { name: "한 점".into(), values: vec![5.0], muted: false }], labels: vec!["11.21".into()] }
            BarList { items: vec![] }
            ShareBar { parts: vec![("a".to_string(), 0)] }
            FunnelChart { steps: vec![] }
            StatCard { label: EVIL, value: "1,844경", delta: -0.5, spark: vec![] }
            EmptyState { icon: "?", title: EVIL, desc: "" }
        });
        assert_escaped(&html);
        assert!(html.contains("▼ 50%"));
        assert!(!html.contains("NaN"));
    }
}

pub mod brand;
pub mod chart;
pub mod icon;
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
        assert!(
            html.contains("&#60;script&#62;"),
            "escaped text missing: {html}"
        );
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
            EmptyState { icon: crate::components::icon::IconName::Search, title: EVIL, desc: "" }
        });
        assert_escaped(&html);
        assert!(html.contains("▼ 50%"));
        assert!(!html.contains("NaN"));
    }

    /// 메뉴 아이콘은 이모지·유니코드 기호가 아니라 장식용 SVG여야 한다 (스크린리더에서 숨김).
    #[test]
    fn nav_icons_are_decorative_svgs() {
        use super::icon::{Icon, IconName};
        for name in IconName::ALL {
            let html = dioxus_ssr::render_element(rsx! { Icon { name } });
            assert!(html.starts_with("<svg"), "{name:?}: {html}");
            assert!(html.contains("aria-hidden=\"true\""), "{name:?}: {html}");
            assert!(html.contains("stroke=\"currentColor\""), "{name:?}: {html}");
            assert!(
                !html.chars().any(|c| (c as u32) > 0x2000),
                "{name:?}: 기호·이모지 금지: {html}"
            );
        }
    }

    /// 22Brix 로고는 대체 텍스트가 있는 이미지로 그린다.
    #[test]
    fn brand_logo_has_alt_text() {
        use super::brand::{BrandLogo, BrandSymbol};
        let html = dioxus_ssr::render_element(rsx! { BrandLogo {} BrandSymbol { size: 32 } });
        assert!(html.contains("alt=\"22Brix\""), "{html}");
        assert!(html.contains("Analytics"), "{html}");
    }
}

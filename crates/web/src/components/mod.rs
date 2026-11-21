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

    /// 프로젝트를 바꾸면(경로의 pid만 바뀜) ProjectScope 아래 화면은 새로 만들어져야 한다.
    /// 그렇지 않으면 처음 프로젝트로 만든 훅 상태(리소스·입력값)가 다음 프로젝트에 남는다.
    #[test]
    fn project_scope_remounts_children_when_pid_changes() {
        use super::ui::ProjectScope;
        use std::cell::Cell;

        thread_local! {
            static PID: Cell<Option<Signal<i32>>> = const { Cell::new(None) };
        }

        #[component]
        fn Probe(pid: i32) -> Element {
            // 처음 만들어질 때의 pid (다시 만들어지지 않으면 그대로 남는다)
            let first = use_hook(|| pid);
            rsx! { "mounted:{first} now:{pid}" }
        }

        #[allow(non_snake_case)]
        fn App() -> Element {
            let pid = use_signal(|| 1);
            PID.with(|c| c.set(Some(pid)));
            rsx! {
                ProjectScope { pid: pid(), Probe { pid: pid() } }
            }
        }

        let mut dom = VirtualDom::new(App);
        dom.rebuild_in_place();
        assert!(dioxus_ssr::render(&dom).contains("mounted:1 now:1"));

        dom.in_runtime(|| {
            let mut s = PID.with(|c| c.get()).unwrap();
            s.set(2);
        });
        dom.render_immediate(&mut dioxus::dioxus_core::NoOpMutations);
        let html = dioxus_ssr::render(&dom);
        assert!(html.contains("mounted:2 now:2"), "{html}");
    }

    /// 이벤트 핸들러가 있는 컴포넌트는 런타임 안에서 그린다.
    fn render_app(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn drawer_is_labelled_modal_dialog() {
        let html = render_app(|| {
            use super::ui::Drawer;
            rsx! { Drawer { open: true, title: "주문 ORD-1", onclose: |_| {}, "본문" } }
        });
        assert!(html.contains("role=\"dialog\""), "{html}");
        assert!(html.contains("aria-modal=\"true\""), "{html}");
        assert!(html.contains("tabindex=\"-1\""), "{html}");
        let id = html
            .split("aria-labelledby=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .expect("aria-labelledby");
        assert!(!id.is_empty());
        assert!(html.contains(&format!("id=\"{id}\"")), "{html}");
    }

    #[test]
    fn empty_state_icon_is_hidden_from_screen_readers() {
        let html = dioxus_ssr::render_element(rsx! {
            EmptyState { icon: crate::components::icon::IconName::Search, title: "없음", desc: "" }
        });
        assert!(
            html.contains("class=\"empty-icon\" aria-hidden=\"true\""),
            "{html}"
        );
    }

    /// 패널 없는 탭 대신 라디오 그룹으로 읽혀야 한다.
    #[test]
    fn segmented_control_is_radio_group() {
        let html = render_app(|| {
            use super::ui::SegmentedControl;
            rsx! {
                SegmentedControl {
                    options: vec![(1, "하나".to_string()), (2, "둘".to_string())],
                    value: 2,
                    onchange: |_| {},
                }
            }
        });
        assert!(html.contains("role=\"radiogroup\""), "{html}");
        assert_eq!(html.matches("role=\"radio\"").count(), 2, "{html}");
        assert_eq!(html.matches("aria-checked=\"true\"").count(), 1, "{html}");
        assert!(
            !html.contains("tablist") && !html.contains("role=\"tab\""),
            "{html}"
        );
    }

    /// 워드마크는 원본 비율(180×50 = 3.6:1)대로 너비·높이를 함께 고정해야 찌그러지지 않는다.
    #[test]
    fn wordmark_keeps_original_aspect_ratio() {
        use super::brand::{BrandLogo, LoginWordmark};
        for html in [
            dioxus_ssr::render_element(rsx! { BrandLogo {} }),
            dioxus_ssr::render_element(rsx! { LoginWordmark {} }),
        ] {
            let attr = |name: &str| -> f64 {
                let key = format!("{name}=\"");
                let start = html.find(&key).unwrap_or_else(|| panic!("{name} 속성 없음: {html}")) + key.len();
                html[start..].split('"').next().unwrap().parse().unwrap()
            };
            let ratio = attr("width") / attr("height");
            assert!((ratio - 3.6).abs() < 0.05, "비율 {ratio}: {html}");
        }
    }
}

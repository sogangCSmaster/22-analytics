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
            StatCard { label: EVIL, value: "1,844경", change: crate::logic::compare::Change::Pct(-0.5), spark: vec![] }
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

    /// 바이너리큐브 로고는 대체 텍스트가 있는 이미지로 그리고, 이전 브랜드 흔적이 없어야 한다.
    #[test]
    fn brand_logo_has_alt_text() {
        use super::brand::{BrandLogo, BrandSymbol, LoginWordmark};
        let html = dioxus_ssr::render_element(
            rsx! { BrandLogo {} BrandSymbol { size: 32 } LoginWordmark {} },
        );
        assert!(html.contains("alt=\"BinaryCube\""), "{html}");
        assert!(html.contains("Analytics"), "{html}");
        assert!(!html.to_lowercase().contains("22brix"), "{html}");
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

    /// 워드마크는 원본 비율(1815×167 ≈ 10.87:1)대로 너비·높이를 함께 고정해야 찌그러지지 않는다.
    #[test]
    fn wordmark_keeps_original_aspect_ratio() {
        use super::brand::{BrandLogo, LoginWordmark};
        for html in [
            dioxus_ssr::render_element(rsx! { BrandLogo {} }),
            dioxus_ssr::render_element(rsx! { LoginWordmark {} }),
        ] {
            let attr = |name: &str| -> f64 {
                let key = format!("{name}=\"");
                let start = html
                    .find(&key)
                    .unwrap_or_else(|| panic!("{name} 속성 없음: {html}"))
                    + key.len();
                html[start..].split('"').next().unwrap().parse().unwrap()
            };
            let ratio = attr("width") / attr("height");
            assert!((ratio - 1815.0 / 167.0).abs() < 0.1, "비율 {ratio}: {html}");
        }
    }

    // -------------------------------------------------------------------
    // 이전 기간 비교: 끄면 증감·이전 값 표시가 하나도 없고, 켜면 보인다.
    // -------------------------------------------------------------------

    use crate::logic::compare::Change;
    use analytics_core::dto::reports::{
        ChannelRevenueRow, ChannelTrafficRow, EventCountRow, FunnelStep, PageRow, ReportRange,
        RevenueReport,
    };

    /// 비교 표시 흔적 (증감 문구 "▲ 12%", 이전 기간 열, 변화 열). 정렬 화살표(▼)만 있는 것은 아니다.
    fn has_compare_markup(html: &str) -> bool {
        html.contains("▲ ")
            || html.contains("▼ ")
            || html.contains("신규")
            || html.contains("이전 기간")
            || html.contains("class=\"chg")
            || html.contains("cmp-")
    }

    #[test]
    fn stat_card_shows_delta_only_with_change() {
        let off = dioxus_ssr::render_element(rsx! {
            StatCard { label: "방문자", value: "1,000", sub: "세션당 2" }
        });
        assert!(!off.contains("delta"), "{off}");
        assert!(!has_compare_markup(&off), "{off}");
        let on = dioxus_ssr::render_element(rsx! {
            StatCard { label: "방문자", value: "1,000", change: Change::Pct(0.25), sub: "이전 800" }
        });
        assert!(on.contains("class=\"delta up\""), "{on}");
        assert!(on.contains("▲ 25%"), "{on}");
        assert!(on.contains("이전 800"), "{on}");
        let pp = dioxus_ssr::render_element(rsx! {
            StatCard { label: "중복률", value: "30%", change: crate::logic::compare::pp(0.30, Some(0.35)) }
        });
        assert!(pp.contains("▼ 5%p"), "{pp}");
    }

    #[test]
    fn change_tag_says_new_instead_of_infinity() {
        use super::ui::ChangeTag;
        let html = dioxus_ssr::render_element(rsx! { ChangeTag { change: Change::New } });
        assert!(html.contains("신규"), "{html}");
        assert!(html.contains("chg new"), "{html}");
        assert!(!html.contains('∞') && !html.contains("inf"), "{html}");
        assert!(
            html.contains("aria-label=\"이전 기간 대비 신규\""),
            "{html}"
        );
    }

    #[test]
    fn barlist_shows_previous_change_only_when_given() {
        let items = vec![("네이버".to_string(), 120), ("메타".to_string(), 30)];
        let off = dioxus_ssr::render_element(rsx! { BarList { items: items.clone() } });
        assert!(!has_compare_markup(&off), "{off}");
        let on = dioxus_ssr::render_element(rsx! {
            BarList { items: items.clone(), prev: vec![Some(100), Some(0)] }
        });
        assert!(on.contains("▲ 20%"), "{on}");
        assert!(on.contains("신규"), "{on}");
    }

    fn ch(channel: &str, sessions: u64, visitors: u64) -> ChannelTrafficRow {
        ChannelTrafficRow {
            channel: channel.into(),
            sessions,
            entrances: sessions,
            visitors,
            pageviews: sessions * 2,
        }
    }

    #[test]
    fn acquisition_table_compare_columns() {
        use crate::logic::table::Sort;
        use crate::pages::acquisition::{ChannelTable, Col};
        let off = render_app(|| {
            let sort = use_signal(|| Sort::desc(Col::Sessions));
            rsx! { ChannelTable { rows: vec![(ch("naver_sa", 100, 80), None)], compare: false, sort } }
        });
        assert!(off.contains("네이버"), "{off}");
        assert!(!has_compare_markup(&off), "{off}");
        let on = render_app(|| {
            let sort = use_signal(|| Sort::desc(Col::Sessions));
            rsx! {
                ChannelTable {
                    rows: vec![
                        (ch("naver_sa", 120, 90), Some(ch("naver_sa", 100, 80))),
                        (ch("tiktok", 5, 5), Some(ch("tiktok", 0, 0))),
                    ],
                    compare: true,
                    sort,
                }
            }
        });
        assert!(on.contains(">이전 기간<"), "{on}");
        assert!(on.contains("변화"), "{on}");
        assert!(on.contains("▲ 20%"), "{on}");
        assert!(on.contains("신규"), "{on}");
        // 모바일 카드 행: 칸마다 이름표
        assert!(on.contains("data-label=\"세션\""), "{on}");
    }

    #[test]
    fn pages_and_events_tables_compare_columns() {
        use crate::logic::table::Sort;
        use crate::pages::events::{Col as ECol, EventsTable};
        use crate::pages::page_report::{Col as PCol, PagesTable};
        fn pr(path: &str, pageviews: u64) -> PageRow {
            PageRow {
                path: path.into(),
                pageviews,
                visitors: 1,
                entrances: 1,
            }
        }
        fn ev(n: &str, count: u64) -> EventCountRow {
            EventCountRow {
                event_name: n.into(),
                count,
                visitors: 5,
            }
        }
        let off = render_app(|| {
            let s1 = use_signal(|| Sort::desc(PCol::Pageviews));
            let s2 = use_signal(|| Sort::desc(ECol::Count));
            rsx! {
                PagesTable { rows: vec![(pr("/a", 10), None)], compare: false, sort: s1 }
                EventsTable { rows: vec![(ev("click", 10), None)], compare: false, sort: s2, onselect: |_| {} }
            }
        });
        assert!(off.contains("/a") && off.contains("click"), "{off}");
        assert!(!has_compare_markup(&off), "{off}");
        let on = render_app(|| {
            let s1 = use_signal(|| Sort::desc(PCol::Pageviews));
            let s2 = use_signal(|| Sort::desc(ECol::Count));
            rsx! {
                PagesTable {
                    // /b: 이전 목록이 잘려 이전 값을 모름
                    rows: vec![(pr("/a", 10), Some(pr("/a", 20))), (pr("/b", 3), None)],
                    compare: true,
                    sort: s1,
                }
                EventsTable { rows: vec![(ev("click", 15), Some(ev("click", 10)))], compare: true, sort: s2, onselect: |_| {} }
            }
        });
        assert!(on.contains("▼ 50%"), "{on}");
        assert!(on.contains("▲ 50%"), "{on}");
        assert_eq!(on.matches(">이전 기간<").count(), 2, "{on}");
    }

    fn step(n: &str, visitors: u64, rate: f64) -> FunnelStep {
        FunnelStep {
            event_name: n.into(),
            visitors,
            rate_from_prev: rate,
            rate_from_first: rate,
        }
    }

    #[test]
    fn funnel_table_shows_previous_conversion_in_pp() {
        use crate::pages::funnel::FunnelTable;
        let off = render_app(|| {
            rsx! {
                FunnelTable { steps: vec![(step("view_item", 100, 1.0), None), (step("purchase", 12, 0.12), None)], compare: false }
            }
        });
        assert!(!has_compare_markup(&off), "{off}");
        let on = render_app(|| {
            rsx! {
                FunnelTable {
                    steps: vec![
                        (step("view_item", 100, 1.0), Some(step("view_item", 80, 1.0))),
                        (step("purchase", 12, 0.12), Some(step("purchase", 8, 0.10))),
                    ],
                    compare: true,
                }
            }
        });
        assert!(on.contains("이전 전환율"), "{on}");
        assert!(on.contains("▲ 2%p"), "{on}");
        assert!(on.contains("▲ 25%"), "{on}");
        assert!(on.contains(">10%<"), "{on}");
    }

    fn rev(attr: i64, spend: i64) -> RevenueReport {
        let roas = (spend > 0).then(|| attr as f64 / spend as f64);
        RevenueReport {
            range: ReportRange {
                from: "2026-11-01".into(),
                to: "2026-11-07".into(),
            },
            model: analytics_core::attribution::Model::LastClick,
            lookback_days: 7,
            raw_sum: attr * 2,
            deduplicated: attr,
            duplicate_amount: attr,
            duplicate_rate: 0.5,
            orders: 3,
            spend_total: spend,
            roas,
            channels: vec![ChannelRevenueRow {
                channel: "meta".into(),
                raw_claimed: attr * 2,
                attributed: attr,
                share: 1.0,
                assists: 1,
                spend,
                roas,
            }],
        }
    }

    #[test]
    fn revenue_channel_table_compare_columns() {
        use crate::pages::revenue::ChannelRevenueTable;
        let off = render_app(|| rsx! { ChannelRevenueTable { report: rev(300, 100), prev: None } });
        assert!(off.contains("메타"), "{off}");
        assert!(!has_compare_markup(&off), "{off}");
        let on = render_app(
            || rsx! { ChannelRevenueTable { report: rev(300, 100), prev: rev(200, 100) } },
        );
        assert!(on.contains(">이전 기간<"), "{on}");
        assert!(on.contains("▲ 50%"), "{on}");
        // ROAS 200% → 300%
        assert!(on.contains("ROAS 변화"), "{on}");
        assert!(on.contains("▲ 100%p"), "{on}");
        // 모바일 카드 행에서는 매체·기여 매출·비중·ROAS만 남긴다
        assert!(on.contains("m-hide"), "{on}");
    }

    #[test]
    fn toast_is_polite_live_region_with_close_button() {
        use super::ui::Toast;
        let html = render_app(|| rsx! { Toast { message: "데모 화면입니다", onclose: |_| {} } });
        assert!(html.contains("aria-live=\"polite\""), "{html}");
        assert!(html.contains("aria-label=\"알림 닫기\""), "{html}");
        assert!(html.contains("데모 화면입니다"), "{html}");
    }

    /// 사이드바에 개발용 "컴포넌트" 메뉴(개발 구역)가 없어야 한다.
    #[test]
    fn sidebar_has_no_component_gallery_link() {
        use super::shell::nav_sections;
        for (pid, admin) in [
            (Some(1), true),
            (Some(1), false),
            (None, true),
            (None, false),
        ] {
            let secs = nav_sections(pid, admin);
            assert!(secs.iter().all(|s| s.title != "개발"), "{pid:?} {admin}");
            for s in &secs {
                for item in &s.items {
                    assert_ne!(item.label, "컴포넌트");
                    assert_ne!(item.to, crate::Route::Gallery {});
                }
            }
        }
        let secs = nav_sections(Some(1), true);
        let labels: Vec<&str> = secs
            .iter()
            .flat_map(|s| s.items.iter().map(|i| i.label))
            .collect();
        assert!(
            labels.contains(&"개요") && labels.contains(&"광고주·사용자"),
            "{labels:?}"
        );
        assert!(
            !nav_sections(Some(1), false)
                .iter()
                .any(|s| s.title == "관리")
        );
    }
}

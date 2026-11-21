//! 메뉴·버튼용 선 아이콘 (24×24, 둥근 끝 1.8px 선).
//!
//! 이모지·유니코드 기호 대신 직접 그린 SVG를 쓴다. 색은 `currentColor`를 따라가므로
//! 메뉴가 활성화되면 글자색(primary)과 함께 바뀐다. 장식용이라 스크린리더에서는 숨긴다.

use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconName {
    Overview,
    Realtime,
    Acquisition,
    Pages,
    Events,
    Funnel,
    Revenue,
    Settings,
    Admin,
    Components,
    Lock,
    Menu,
    Logout,
    Search,
    Inbox,
}

impl IconName {
    #[cfg_attr(not(test), allow(dead_code))]
    pub const ALL: [IconName; 15] = [
        IconName::Overview,
        IconName::Realtime,
        IconName::Acquisition,
        IconName::Pages,
        IconName::Events,
        IconName::Funnel,
        IconName::Revenue,
        IconName::Settings,
        IconName::Admin,
        IconName::Components,
        IconName::Lock,
        IconName::Menu,
        IconName::Logout,
        IconName::Search,
        IconName::Inbox,
    ];
}

#[component]
pub fn Icon(name: IconName, #[props(default = 20)] size: u32) -> Element {
    let body = match name {
        // 대시보드 타일
        IconName::Overview => rsx! {
            rect { x: "3.5", y: "3.5", width: "7", height: "8", rx: "2" }
            rect { x: "13.5", y: "3.5", width: "7", height: "5", rx: "2" }
            rect { x: "13.5", y: "11.5", width: "7", height: "9", rx: "2" }
            rect { x: "3.5", y: "14.5", width: "7", height: "6", rx: "2" }
        },
        // 맥박선
        IconName::Realtime => rsx! {
            path { d: "M3 12h3.5l2.5-6.5 5 13 2.5-6.5H21" }
        },
        // 나침반: 유입 경로
        IconName::Acquisition => rsx! {
            circle { cx: "12", cy: "12", r: "8.5" }
            path { d: "M15.2 8.8l-1.9 4.5-4.5 1.9 1.9-4.5z" }
        },
        // 문서
        IconName::Pages => rsx! {
            path { d: "M7.5 3.5h6.2l4.8 4.8V19a1.5 1.5 0 0 1-1.5 1.5h-9.5A1.5 1.5 0 0 1 6 19V5a1.5 1.5 0 0 1 1.5-1.5z" }
            path { d: "M13.5 3.5V8.5h5" }
            path { d: "M9 12.5h6M9 16h4" }
        },
        // 클릭 포인터
        IconName::Events => rsx! {
            path { d: "M9.5 9.5l10 3.8-4.3 1.7-1.7 4.3z" }
            path { d: "M9.5 3.5v2.2M3.5 9.5h2.2M5.3 5.3l1.5 1.5" }
        },
        // 깔때기
        IconName::Funnel => rsx! {
            path { d: "M4 5.5h16l-6.2 7.2v5.4l-3.6 1.6v-7z" }
        },
        // 지갑
        IconName::Revenue => rsx! {
            path { d: "M5.5 7.5V6.5a2 2 0 0 1 2-2h9.5v3" }
            rect { x: "3.5", y: "7.5", width: "17", height: "12", rx: "2.5" }
            path { d: "M16 13.5h2" }
        },
        // 조절 슬라이더
        IconName::Settings => rsx! {
            path { d: "M4 7.5h8.5M16.5 7.5H20M4 16.5h3.5M11.5 16.5H20" }
            circle { cx: "14.5", cy: "7.5", r: "2" }
            circle { cx: "9.5", cy: "16.5", r: "2" }
        },
        // 사람 둘
        IconName::Admin => rsx! {
            circle { cx: "9.5", cy: "8.5", r: "3.2" }
            path { d: "M3.5 19.5c.8-3.2 3.1-4.8 6-4.8s5.2 1.6 6 4.8" }
            path { d: "M15.5 5.3a3.2 3.2 0 0 1 0 6.3M17.6 14.9c1.6.7 2.6 2.2 2.9 4.6" }
        },
        // 겹친 레이어
        IconName::Components => rsx! {
            path { d: "M12 3.5l8.5 4.6-8.5 4.6-8.5-4.6z" }
            path { d: "M3.5 12.4l8.5 4.6 8.5-4.6" }
            path { d: "M3.5 16.4l8.5 4.6 8.5-4.6" }
        },
        // 자물쇠
        IconName::Lock => rsx! {
            rect { x: "5", y: "10.5", width: "14", height: "10", rx: "2.5" }
            path { d: "M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5" }
        },
        // 햄버거 메뉴
        IconName::Menu => rsx! {
            path { d: "M4 7h16M4 12h16M4 17h16" }
        },
        // 로그아웃
        IconName::Logout => rsx! {
            path { d: "M14 4.5h3.5A1.5 1.5 0 0 1 19 6v12a1.5 1.5 0 0 1-1.5 1.5H14" }
            path { d: "M10.5 8.5L7 12l3.5 3.5M7 12h8.5" }
        },
        // 돋보기
        IconName::Search => rsx! {
            circle { cx: "11", cy: "11", r: "6.5" }
            path { d: "M16 16l4 4" }
        },
        // 받은편지함
        IconName::Inbox => rsx! {
            path { d: "M4 13.5l2.4-7.2A1.5 1.5 0 0 1 7.8 5.3h8.4a1.5 1.5 0 0 1 1.4 1l2.4 7.2" }
            path { d: "M4 13.5V18a1.5 1.5 0 0 0 1.5 1.5h13A1.5 1.5 0 0 0 20 18v-4.5h-4.5l-1.2 2h-4.6l-1.2-2z" }
        },
    };
    rsx! {
        svg {
            class: "icon",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.8",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            "focusable": "false",
            {body}
        }
    }
}

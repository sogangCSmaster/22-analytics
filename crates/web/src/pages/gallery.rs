//! 컴포넌트 갤러리: 디자인 토큰과 기본 컴포넌트를 한 화면에서 확인한다.

use crate::components::chart::{BarList, SplitBar, TrendChart};
use crate::components::ui::*;
use dioxus::prelude::*;

#[component]
pub fn Gallery() -> Element {
    let mut seg = use_signal(|| 1u8);
    let mut on = use_signal(|| true);
    let mut text = use_signal(String::new);
    let mut drawer = use_signal(|| false);

    rsx! {
        div { class: "page-head",
            div {
                h1 { class: "page-title", "컴포넌트" }
                p { class: "page-desc", "디자인 토큰과 기본 컴포넌트 (docs/ARCHITECTURE.md §12.1)" }
            }
        }

        Card { title: "버튼",
            div { class: "row",
                Button { "기본" }
                Button { variant: Variant::Secondary, "보조" }
                Button { variant: Variant::Ghost, "회색" }
                Button { variant: Variant::Danger, "삭제" }
                Button { disabled: true, "비활성" }
                Button { size: Size::Sm, "작게" }
                Button { size: Size::Lg, "크게" }
            }
        }

        Card { title: "배지",
            div { class: "row",
                Badge { "기본" }
                Badge { tone: Tone::Primary, "권장" }
                Badge { tone: Tone::Up, "정상" }
                Badge { tone: Tone::Down, "오류" }
                Badge { tone: Tone::Warn, "예시 데이터" }
            }
        }

        div { class: "grid grid-3",
            StatCard { label: "매체 합산 매출", value: "2억 원", sub: "ROAS 500%", style: StatStyle::Struck }
            StatCard { label: "실제 매출", value: "1.3억 원", sub: "주문번호 기준", style: StatStyle::Hero }
            StatCard { label: "방문자", value: "48,210", delta: 0.082, sub: "지난 기간 대비" }
        }

        div { class: "grid grid-2",
            Card { title: "입력",
                div { style: "display: flex; flex-direction: column; gap: 16px;",
                    SegmentedControl {
                        options: vec![(0u8, "오늘".to_string()), (1, "7일".to_string()), (2, "30일".to_string())],
                        value: seg(),
                        onchange: move |v| seg.set(v),
                    }
                    ToggleRow { title: "토글", desc: "설명 문구", on: on(), onchange: move |v| on.set(v) }
                    TextField { label: "텍스트 입력", value: text(), oninput: move |v| text.set(v), placeholder: "입력하세요", hint: "도움말" }
                    TextField { label: "오류 상태", value: "https://", oninput: move |_| {}, error: "도메인만 입력하세요" }
                }
            }
            Card { title: "로딩 · 빈 상태",
                div { style: "display: flex; flex-direction: column; gap: 12px;",
                    Skeleton { width: "40%", height: "14px" }
                    Skeleton { height: "32px" }
                    Skeleton { width: "70%" }
                }
                EmptyState {
                    icon: "📭",
                    title: "아직 받은 데이터가 없어요",
                    desc: "스크립트를 설치하면 몇 초 안에 여기에 나타나요.",
                    action: rsx! { Button { variant: Variant::Secondary, "설치 방법 보기" } },
                }
            }
        }

        Card { title: "차트",
            TrendChart {
                values: vec![120, 180, 160, 240, 210, 300, 280],
                labels: ["월", "화", "수", "목", "금", "토", "일"].iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                height: 180,
            }
            div { style: "margin-top: 24px;",
                BarList { items: vec![("네이버".to_string(), 120), ("메타".to_string(), 80), ("구글".to_string(), 40)] }
            }
            div { style: "margin-top: 24px;",
                SplitBar {
                    parts: vec![
                        ("네이버".to_string(), 50_000, "var(--chart-1)".to_string()),
                        ("메타".to_string(), 50_000, "var(--chart-2)".to_string()),
                    ],
                }
            }
        }

        Card { title: "코드 · 패널",
            CodeBlock { code: "at('event', 'signup_complete', {{ plan: 'pro' }});" }
            div { style: "margin-top: 16px;",
                Button { variant: Variant::Ghost, onclick: move |_| drawer.set(true), "상세 패널 열기" }
            }
        }

        Drawer { open: drawer(), title: "상세 패널", onclose: move |_| drawer.set(false),
            p { "목록 맥락을 유지한 채 상세 정보를 보여줍니다." }
        }
    }
}

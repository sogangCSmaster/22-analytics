//! 설치·수집 설정 화면. Phase 1에서 api와 연결한다 (지금은 화면 상태만 바뀐다).

use crate::components::ui::{
    Badge, Button, Card, CodeBlock, TextField, Toast, ToggleRow, Tone, Variant, sleep_ms,
};
use dioxus::prelude::*;

const PUBLIC_KEY: &str = "pk_devlocal0000000000000000";

#[component]
pub fn Install() -> Element {
    let mut auto_pv = use_signal(|| true);
    let mut spa = use_signal(|| true);
    let mut ecommerce = use_signal(|| true);
    let mut outbound = use_signal(|| true);
    let mut scroll = use_signal(|| true);
    let mut downloads = use_signal(|| false);
    let mut domain = use_signal(String::new);
    let mut toast = use_signal(|| None::<String>);

    let domain_error = {
        let d = domain();
        (!d.is_empty() && (d.contains("://") || d.contains('/') || !d.contains('.')))
            .then(|| "도메인만 입력하세요. 예: shop.co.kr".to_string())
    };

    let snippet = format!(
        "<script>window.at=window.at||function(){{(at.q=at.q||[]).push(arguments)}};</script>\n<script async src=\"https://track.광고주도메인/t.js\" data-key=\"{PUBLIC_KEY}\"></script>"
    );
    let s2s = "POST https://track.광고주도메인/v1/orders\nAuthorization: Bearer sk_live_...\n\n{\n  \"event_type\": \"PURCHASE_CONFIRMED\",\n  \"order_id\": \"ORD-20261120-009901\",\n  \"visitor_id\": \"<_at_vid 쿠키 값>\",\n  \"net_amount\": 100000,\n  \"ordered_at\": \"2026-11-20T18:45:12+09:00\"\n}";

    rsx! {
        div { class: "page-head",
            div {
                h1 { class: "page-title", "설치·수집 설정" }
                p { class: "page-desc", "사이트에 스크립트를 넣고, 무엇을 수집할지 정합니다." }
            }
            Button {
                onclick: move |_| {
                    toast.set(Some("최근 10분 안에 받은 이벤트가 있어요".into()));
                    spawn(async move {
                        sleep_ms(1800).await;
                        toast.set(None);
                    });
                },
                "설치 확인"
            }
        }

        Card { title: "1. 스크립트 넣기",
            p { class: "page-desc", style: "margin: 0 0 16px;",
                "모든 페이지의 "
                code { "<head>" }
                " 안에 아래 코드를 붙여 넣으세요."
            }
            CodeBlock { code: snippet }
        }

        Card { title: "2. 주문은 서버에서 보내기",
            actions: rsx! { Badge { tone: Tone::Primary, "권장" } },
            p { class: "page-desc", style: "margin: 0 0 16px;",
                "브라우저는 결제 직후 창을 닫으면 기록을 놓치지만, 주문 서버는 결제 사실을 반드시 압니다."
            }
            CodeBlock { code: s2s.to_string() }
        }

        div { class: "grid grid-2",
            Card { title: "수집 항목",
                ToggleRow { title: "페이지뷰 자동 수집", desc: "페이지를 열 때마다 기록합니다.", on: auto_pv(), onchange: move |v| auto_pv.set(v) }
                ToggleRow { title: "SPA 페이지 이동", desc: "새로고침 없이 주소가 바뀌는 사이트용입니다.", on: spa(), onchange: move |v| spa.set(v) }
                ToggleRow { title: "전자상거래", desc: "상품 조회, 장바구니, 결제 시작, 구매 이벤트를 받습니다.", on: ecommerce(), onchange: move |v| ecommerce.set(v) }
                ToggleRow { title: "외부 링크 클릭", on: outbound(), onchange: move |v| outbound.set(v) }
                ToggleRow { title: "스크롤 깊이", desc: "25 · 50 · 75 · 90%", on: scroll(), onchange: move |v| scroll.set(v) }
                ToggleRow { title: "파일 다운로드", desc: "pdf, zip, xlsx, hwp 등", on: downloads(), onchange: move |v| downloads.set(v) }
            }
            Card { title: "수집 허용 도메인",
                div { style: "display: flex; flex-direction: column; gap: 16px;",
                    TextField {
                        label: "도메인 추가",
                        value: domain(),
                        oninput: move |v| domain.set(v),
                        placeholder: "shop.co.kr",
                        hint: "하위 도메인(www, m 등)은 자동으로 포함돼요.",
                        error: domain_error.clone(),
                    }
                    div { class: "row",
                        Badge { "localhost" }
                        Badge { "127.0.0.1" }
                    }
                    div {
                        Button { variant: Variant::Secondary, disabled: domain().is_empty() || domain_error.is_some(), "추가" }
                    }
                }
            }
        }

        if let Some(msg) = toast() {
            Toast { message: msg }
        }
    }
}

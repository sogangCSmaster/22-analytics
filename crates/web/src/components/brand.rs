//! 바이너리큐브(BinaryCube) 브랜드 요소: 공식 워드마크(딥그린·흰색)와 B 심볼.
//! 파일 출처: http://www.binarycube.co.kr (assets/brand/). 원본 1815×167 → 높이 72px로 축소.
//! 심볼은 워드마크의 첫 글자 B를 딥그린 라운드 사각형 위에 올려 만들었다 (공식 파비콘 없음).

use dioxus::prelude::*;

pub const LOGO: Asset = asset!("/assets/brand/binarycube-logo.png");
pub const LOGO_WHITE: Asset = asset!("/assets/brand/binarycube-logo-white.png");
pub const SYMBOL: Asset = asset!("/assets/brand/binarycube-symbol.png");
pub const FAVICON: Asset = asset!("/assets/brand/favicon.png");

/// 사이드바 상단 브랜드: 딥그린 워드마크 + 제품명. 워드마크 비율 1815:167 ≈ 10.87:1.
#[component]
pub fn BrandLogo() -> Element {
    rsx! {
        div { class: "brand",
            img { class: "brand-wordmark", src: LOGO, alt: "BinaryCube", width: "163", height: "15" }
            span { class: "brand-product", "Analytics" }
        }
    }
}

/// 로그인 화면 브랜드 패널(딥그린 배경)의 흰색 워드마크.
#[component]
pub fn LoginWordmark() -> Element {
    rsx! {
        img { class: "brand-wordmark login-wordmark", src: LOGO_WHITE, alt: "BinaryCube", width: "261", height: "24" }
    }
}

/// B 심볼 (로그인 카드·로딩 화면 등 작은 자리).
#[component]
pub fn BrandSymbol(#[props(default = 32)] size: u32) -> Element {
    rsx! {
        img { class: "brand-symbol", src: SYMBOL, alt: "BinaryCube", width: "{size}", height: "{size}" }
    }
}

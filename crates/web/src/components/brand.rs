//! 22Brix 브랜드 요소: 공식 로고(흰색 워드마크)와 B 심볼.
//! 파일 출처: https://22brix.co.kr (assets/brand/)

use dioxus::prelude::*;

pub const LOGO_WHITE: Asset = asset!("/assets/brand/22brix-logo-white.png");
pub const SYMBOL: Asset = asset!("/assets/brand/22brix-symbol.png");
pub const FAVICON: Asset = asset!("/assets/brand/favicon.png");

/// 사이드바 상단 브랜드 타일: 브랜드 그라데이션 위에 흰색 워드마크 + 제품명.
#[component]
pub fn BrandLogo() -> Element {
    rsx! {
        div { class: "brand",
            img { class: "brand-wordmark", src: LOGO_WHITE, alt: "22Brix", width: "72", height: "20" }
            span { class: "brand-product", "Analytics" }
        }
    }
}

/// 로그인 화면 브랜드 패널의 워드마크. 공식 사이트와 같은 크기(115×32, 원본 비율 3.6:1).
#[component]
pub fn LoginWordmark() -> Element {
    rsx! {
        img { class: "brand-wordmark login-wordmark", src: LOGO_WHITE, alt: "22Brix", width: "115", height: "32" }
    }
}

/// 그라데이션 B 심볼 (로딩·빈 화면 등 작은 자리).
#[component]
pub fn BrandSymbol(#[props(default = 32)] size: u32) -> Element {
    rsx! {
        img { class: "brand-symbol", src: SYMBOL, alt: "22Brix", width: "{size}", height: "{size}" }
    }
}

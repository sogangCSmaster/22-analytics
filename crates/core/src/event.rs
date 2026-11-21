//! 수집 이벤트: 트래커가 보내는 전송 포맷(`Wire*`)과 정규화된 저장 포맷(`Event`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

/// 배치 한도. 트래커와 Worker가 같은 값을 쓴다.
pub const MAX_EVENTS_PER_BATCH: usize = 50;
pub const MAX_BATCH_BYTES: usize = 64 * 1024;
pub const MAX_PROPS: usize = 50;
pub const MAX_PROP_KEY_LEN: usize = 64;
pub const MAX_PROP_VALUE_LEN: usize = 500;
pub const MAX_URL_LEN: usize = 2048;
pub const MAX_ITEMS: usize = 100;

/// `POST /v1/collect` 본문.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireBatch {
    /// public key
    pub k: String,
    /// visitor 후보 ID (트래커가 만든 값)
    #[serde(default)]
    pub v: Option<String>,
    /// session_id
    #[serde(default)]
    pub s: Option<String>,
    /// identify()로 받은 회원 해시 (sha256 hex)
    #[serde(default)]
    pub m: Option<String>,
    /// 트래커 버전
    #[serde(default)]
    pub sv: u32,
    /// 세션 랜딩 정보. 세션의 모든 이벤트가 같은 채널·캠페인을 갖도록 한다 (GA의 세션 범위 소스).
    #[serde(default)]
    pub l: Option<WireLanding>,
    pub e: Vec<WireEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireLanding {
    /// 세션을 시작한 페이지 URL
    pub u: String,
    /// 세션을 시작한 페이지의 referrer
    #[serde(default)]
    pub r: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireEvent {
    /// event_id (재전송 중복 방지용)
    #[serde(default)]
    pub id: Option<String>,
    /// 이벤트 이름
    pub n: String,
    /// 클라이언트 시각 (epoch ms)
    pub t: i64,
    /// 페이지 URL
    pub u: String,
    /// referrer
    #[serde(default)]
    pub r: Option<String>,
    /// 페이지 제목
    #[serde(default)]
    pub ti: Option<String>,
    /// 화면 너비
    #[serde(default)]
    pub sw: Option<u32>,
    /// 세션 시작 이벤트 여부
    #[serde(default)]
    pub ss: Option<bool>,
    /// 커스텀 속성
    #[serde(default)]
    pub p: Option<BTreeMap<String, serde_json::Value>>,
    /// 전자상거래 페이로드
    #[serde(default)]
    pub ec: Option<WireEcommerce>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WireEcommerce {
    #[serde(default)]
    pub order_id: Option<String>,
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub items: Vec<WireItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WireItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub price: f64,
    #[serde(default = "one")]
    pub qty: u32,
}

fn one() -> u32 {
    1
}

/// 정규화된 이벤트. ingest가 ClickHouse `events` 행으로 변환해 저장한다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub project_id: u32,
    pub event_id: Uuid,
    pub ts_ms: i64,
    pub received_at_ms: i64,
    pub event_name: String,
    pub visitor_id: Uuid,
    pub session_id: Uuid,
    pub is_session_start: bool,
    pub member_hash: String,

    pub url: String,
    pub host: String,
    pub path: String,
    pub title: String,
    pub referrer: String,
    pub referrer_host: String,

    pub utm_source: String,
    pub utm_medium: String,
    pub utm_campaign: String,
    pub utm_content: String,
    pub utm_term: String,
    pub click_id: String,
    pub click_id_type: String,
    pub channel: String,

    pub country: String,
    pub region: String,
    pub city: String,
    pub device_type: String,
    pub browser: String,
    pub os: String,
    pub screen_w: u16,

    pub props: Vec<(String, String)>,
    pub ec: Option<Ecommerce>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Ecommerce {
    pub order_id: String,
    /// 통화 최소단위 정수 (KRW = 원)
    pub value: i64,
    pub currency: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub price: i64,
    pub qty: u32,
}

/// 예약 이벤트 이름.
pub mod names {
    pub const PAGE_VIEW: &str = "page_view";
    pub const OUTBOUND_CLICK: &str = "outbound_click";
    pub const FILE_DOWNLOAD: &str = "file_download";
    pub const SCROLL: &str = "scroll";

    /// GA4 호환 전자상거래 이벤트.
    pub const ECOMMERCE: &[&str] = &[
        "view_item_list",
        "select_item",
        "view_item",
        "add_to_cart",
        "remove_from_cart",
        "view_cart",
        "begin_checkout",
        "add_shipping_info",
        "add_payment_info",
        "purchase",
        "refund",
    ];

    pub fn is_ecommerce(name: &str) -> bool {
        ECOMMERCE.contains(&name)
    }
}

/// 이벤트 이름 규칙: 소문자·숫자·밑줄, 1~64자, 밑줄로 시작하지 않음.
pub fn is_valid_event_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('_')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_name_rules() {
        assert!(is_valid_event_name("signup_complete"));
        assert!(is_valid_event_name("step2"));
        assert!(!is_valid_event_name("SignUp"));
        assert!(!is_valid_event_name("_internal"));
        assert!(!is_valid_event_name(""));
        assert!(!is_valid_event_name("a b"));
    }

    #[test]
    fn wire_batch_minimal_json() {
        let b: WireBatch = serde_json::from_str(
            r#"{"k":"pk_devlocal000","e":[{"n":"page_view","t":1,"u":"https://a.com/"}]}"#,
        )
        .unwrap();
        assert_eq!(b.e.len(), 1);
        assert!(b.v.is_none());
    }
}

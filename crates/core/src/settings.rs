//! 프로젝트 수집 설정. Postgres `projects.settings`(jsonb)와 KV에 같은 JSON으로 저장된다.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackingSettings {
    pub auto_pageview: bool,
    pub spa_mode: bool,
    pub outbound_links: bool,
    pub file_downloads: bool,
    pub scroll_depth: bool,
    pub ecommerce: bool,
    pub client_purchase_as_order: bool,
    pub click_id_params: Vec<String>,
    pub capture_query_params: Vec<String>,
    pub strip_query_params: Vec<String>,
    pub respect_dnt: bool,
}

/// 개인정보가 섞일 수 있어 항상 제거하는 쿼리 파라미터.
pub const ALWAYS_STRIP_PARAMS: &[&str] = &[
    "email",
    "e-mail",
    "mail",
    "phone",
    "tel",
    "mobile",
    "token",
    "access_token",
    "password",
    "pw",
    "name",
    "address",
    "ssn",
];

pub const DEFAULT_CLICK_ID_PARAMS: &[&str] = &[
    "gclid",
    "fbclid",
    "abx_tid",
    "click_id",
    "n_keyword",
    "NaPm",
    "_a1_kad",
    "wbraid",
    "gbraid",
    "ttclid",
];

impl Default for TrackingSettings {
    fn default() -> Self {
        Self {
            auto_pageview: true,
            spa_mode: true,
            outbound_links: false,
            file_downloads: false,
            scroll_depth: false,
            ecommerce: false,
            client_purchase_as_order: false,
            click_id_params: DEFAULT_CLICK_ID_PARAMS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            capture_query_params: vec![],
            strip_query_params: vec![],
            respect_dnt: false,
        }
    }
}

impl TrackingSettings {
    pub fn should_strip(&self, key: &str) -> bool {
        let k = key.to_ascii_lowercase();
        ALWAYS_STRIP_PARAMS.contains(&k.as_str())
            || self
                .strip_query_params
                .iter()
                .any(|p| p.eq_ignore_ascii_case(key))
    }
}

/// 트래커에 내려주는 공개 설정 (`GET /v1/config`). 비밀 정보는 절대 넣지 않는다.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PublicConfig {
    pub v: u32,
    pub pv: bool,
    pub spa: bool,
    pub out: bool,
    pub dl: bool,
    pub scr: bool,
    pub dnt: bool,
}

impl TrackingSettings {
    pub fn public_config(&self, version: u32) -> PublicConfig {
        PublicConfig {
            v: version,
            pv: self.auto_pageview,
            spa: self.spa_mode,
            out: self.outbound_links,
            dl: self.file_downloads,
            scr: self.scroll_depth,
            dnt: self.respect_dnt,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_fills_defaults() {
        let s: TrackingSettings = serde_json::from_str(r#"{"ecommerce": true}"#).unwrap();
        assert!(s.ecommerce);
        assert!(s.auto_pageview);
        assert!(s.click_id_params.iter().any(|p| p == "gclid"));
    }

    #[test]
    fn strips_sensitive_params() {
        let s = TrackingSettings {
            strip_query_params: vec!["coupon".into()],
            ..Default::default()
        };
        assert!(s.should_strip("Email"));
        assert!(s.should_strip("coupon"));
        assert!(!s.should_strip("utm_source"));
    }
}

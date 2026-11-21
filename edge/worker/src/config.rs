//! 프로젝트 설정: KV 키, Worker 메모리 캐시(60초), `GET /v1/config` 응답.

use crate::http::EdgeResponse;
use analytics_core::PublicKey;
use analytics_core::collect::ProjectConfig;
use std::collections::HashMap;

pub const CONFIG_TTL_MS: i64 = 60_000;
/// 무작위 키로 메모리를 채우는 공격을 막기 위한 캐시 상한 (isolate당).
pub const CONFIG_CACHE_MAX: usize = 10_000;
/// 공개 설정 응답 캐시 (ARCHITECTURE §3.1: 설정 변경은 5분 안에 반영)
pub const PUBLIC_CONFIG_MAX_AGE: u32 = 300;
/// 없는 키에 대한 404도 잠시 캐시해 KV 조회를 줄인다.
pub const NOT_FOUND_MAX_AGE: u32 = 60;

/// KV `PROJECTS`의 키: `project:{public_key}`
pub fn kv_key(key: &PublicKey) -> String {
    format!("project:{key}")
}

/// 쿼리 문자열에서 첫 `k`를 꺼내 공개 키 형식인지 확인한다.
pub fn key_from_query(query: Option<&str>) -> Option<PublicKey> {
    let (_, k) = url::form_urlencoded::parse(query?.as_bytes()).find(|(name, _)| name == "k")?;
    PublicKey::parse(&k).ok()
}

/// 공개 키별 `ProjectConfig` 캐시. 없는 키(`None`)도 캐시한다 (negative caching).
#[derive(Debug)]
pub struct ConfigCache {
    ttl_ms: i64,
    max_entries: usize,
    entries: HashMap<String, (i64, Option<ProjectConfig>)>,
}

impl Default for ConfigCache {
    fn default() -> Self {
        Self::new(CONFIG_TTL_MS, CONFIG_CACHE_MAX)
    }
}

impl ConfigCache {
    pub fn new(ttl_ms: i64, max_entries: usize) -> Self {
        Self {
            ttl_ms,
            max_entries,
            entries: HashMap::new(),
        }
    }

    /// 신선한 항목이 있으면 `Some(설정 또는 None)`, 없거나 만료됐으면 `None`.
    pub fn get(&self, key: &str, now_ms: i64) -> Option<Option<ProjectConfig>> {
        self.entries
            .get(key)
            .filter(|(at, _)| now_ms - at < self.ttl_ms)
            .map(|(_, cfg)| cfg.clone())
    }

    pub fn put(&mut self, key: &str, cfg: Option<ProjectConfig>, now_ms: i64) {
        if self.entries.len() >= self.max_entries && !self.entries.contains_key(key) {
            let ttl = self.ttl_ms;
            self.entries.retain(|_, (at, _)| now_ms - *at < ttl);
            if self.entries.len() >= self.max_entries {
                self.entries.clear();
            }
        }
        self.entries.insert(key.to_owned(), (now_ms, cfg));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// `GET /v1/config` 응답. 비활성·없는 프로젝트는 404.
pub fn config_response(cfg: Option<&ProjectConfig>) -> EdgeResponse {
    let public = cfg
        .filter(|p| p.active)
        .map(|p| p.settings.public_config(p.settings_version))
        .and_then(|pc| serde_json::to_vec(&pc).ok());
    let (status, max_age, body) = match public {
        Some(body) => (200, PUBLIC_CONFIG_MAX_AGE, body),
        None => (404, NOT_FOUND_MAX_AGE, vec![]),
    };
    let mut r = EdgeResponse::empty(status)
        .with_header("cache-control", format!("public, max-age={max_age}"))
        // 공개 정보이고 credentials가 필요 없으므로 `*` (Vary 없이 엣지 캐시 가능)
        .with_header("access-control-allow-origin", "*");
    if status == 200 {
        r = r.with_header("content-type", "application/json; charset=utf-8");
    }
    r.body = body;
    r
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use analytics_core::settings::TrackingSettings;

    pub(crate) fn project() -> ProjectConfig {
        ProjectConfig {
            project_id: 7,
            public_key: "pk_testtest01".into(),
            active: true,
            allowed_origins: vec!["client.com".into()],
            settings: TrackingSettings::default(),
            settings_version: 3,
            channel_rules: vec![],
            salt_hex: "deadbeefsalt".into(),
        }
    }

    #[test]
    fn kv_key_format() {
        let k = PublicKey::parse("pk_testtest01").unwrap();
        assert_eq!(kv_key(&k), "project:pk_testtest01");
    }

    #[test]
    fn key_from_query_validates() {
        assert_eq!(
            key_from_query(Some("k=pk_testtest01")).map(|k| k.to_string()),
            Some("pk_testtest01".into())
        );
        assert_eq!(
            key_from_query(Some("x=1&k=pk_testtest01&k=pk_other0000")).map(|k| k.to_string()),
            Some("pk_testtest01".into())
        );
        assert!(key_from_query(None).is_none());
        assert!(key_from_query(Some("")).is_none());
        assert!(key_from_query(Some("k=")).is_none());
        assert!(key_from_query(Some("k=sk_live_abcdefgh")).is_none());
        // KV 키 조작 시도 (다른 접두사로 탈출)
        assert!(key_from_query(Some("k=pk_aaaaaaaa%3Aadmin")).is_none());
        assert!(key_from_query(Some("k=pk_aaaaaaaa/../x")).is_none());
    }

    #[test]
    fn public_config_response() {
        let r = config_response(Some(&project()));
        assert_eq!(r.status, 200);
        assert_eq!(r.header("cache-control"), Some("public, max-age=300"));
        assert_eq!(r.header("access-control-allow-origin"), Some("*"));
        let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["v"], 3);
        assert_eq!(v["pv"], true);
        // 비밀 값은 절대 내려가지 않는다
        let s = String::from_utf8(r.body).unwrap();
        assert!(!s.contains("deadbeefsalt"));
        assert!(!s.contains("client.com"));
        assert!(!s.contains("project_id"));
    }

    #[test]
    fn unknown_or_inactive_is_404() {
        let r = config_response(None);
        assert_eq!(r.status, 404);
        assert_eq!(r.header("cache-control"), Some("public, max-age=60"));
        let mut p = project();
        p.active = false;
        assert_eq!(config_response(Some(&p)).status, 404);
    }

    #[test]
    fn cache_ttl() {
        let mut c = ConfigCache::new(60_000, 10);
        assert_eq!(c.get("pk_a", 0), None);
        c.put("pk_a", Some(project()), 1_000);
        assert_eq!(c.get("pk_a", 1_000), Some(Some(project())));
        assert_eq!(c.get("pk_a", 60_999), Some(Some(project())));
        assert_eq!(
            c.get("pk_a", 61_000),
            None,
            "60초가 지나면 KV를 다시 읽는다"
        );
        c.put("pk_missing", None, 0);
        assert_eq!(c.get("pk_missing", 10), Some(None), "없는 키도 캐시");
    }

    #[test]
    fn cache_is_bounded() {
        let mut c = ConfigCache::new(60_000, 3);
        for i in 0..3 {
            c.put(&format!("pk_{i}"), None, 0);
        }
        assert_eq!(c.len(), 3);
        // 만료된 항목부터 비운다
        c.put("pk_new", None, 100_000);
        assert_eq!(c.len(), 1);
        assert_eq!(c.get("pk_new", 100_000), Some(None));
        // 모두 신선한데 가득 차면 통째로 비우고 새로 담는다
        for i in 0..10_000 {
            c.put(&format!("pk_x{i}"), None, 100_000);
            assert!(c.len() <= 3);
        }
    }
}

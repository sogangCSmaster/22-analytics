//! 프로젝트 설정: KV 키, Worker 메모리 캐시(5분), `GET /v1/config` 응답.

use crate::http::EdgeResponse;
use analytics_core::PublicKey;
use analytics_core::cache::TtlCache;
use analytics_core::collect::ProjectConfig;

/// 설정 변경은 KV 반영 후 최대 5분 안에 적용된다 (공개 설정 응답 max-age와 같다).
pub const CONFIG_TTL_MS: i64 = 300_000;
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
pub type ConfigCache = TtlCache<String, ProjectConfig>;

pub fn new_cache() -> ConfigCache {
    TtlCache::new(CONFIG_TTL_MS, CONFIG_CACHE_MAX)
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
    fn memory_cache_matches_public_config_max_age() {
        // C2: 설정 캐시는 5분 (공개 설정 응답 max-age와 같다). isolate당 KV 읽기를 줄인다.
        assert_eq!(CONFIG_TTL_MS, 300_000);
        assert_eq!(CONFIG_TTL_MS, i64::from(PUBLIC_CONFIG_MAX_AGE) * 1000);
    }

    #[test]
    fn cache_uses_config_ttl() {
        let mut c = new_cache();
        c.put("pk_a".into(), Some(project()), 0);
        assert_eq!(c.get("pk_a", CONFIG_TTL_MS - 1), Some(Some(project())));
        assert_eq!(c.get("pk_a", CONFIG_TTL_MS), None);
    }
}

//! S2S API 키 DTO.
//!
//! TODO(WP6): `analytics_core::dto::admin::{ApiKeyDto, CreatedApiKeyResponse}`가 추가되면
//! 이 임시 정의를 지우고 core 타입을 쓴다. 필드 이름이 확정되기 전이라 읽기를 너그럽게 한다
//! (모르는 필드 무시, 빠진 필드 기본값, 평문 키는 응답 어디에 있든 `sk_live_`로 찾는다).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiKeyDto {
    pub id: i64,
    /// 표시용 `sk_live_XXXXXXXX`
    #[serde(alias = "key_prefix")]
    pub prefix: String,
    #[serde(alias = "name")]
    pub label: String,
    pub scopes: Vec<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

/// `GET /v1/projects/{id}/api-keys`: 배열 또는 `{items: [...]}`/`{keys: [...]}`
pub fn parse_list(v: &serde_json::Value) -> Vec<ApiKeyDto> {
    todo!()
}

/// `POST` 응답에서 한 번만 내려오는 평문 키와 메타데이터.
pub fn parse_created(v: &serde_json::Value) -> Option<(String, ApiKeyDto)> {
    todo!()
}

/// 화면 표시: `sk_live_AbCdEfGh••••••••`
pub fn masked(prefix: &str) -> String {
    format!("{prefix}••••••••")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn list_shapes() {
        let item = json!({"id": 1, "prefix": "sk_live_abcdefgh", "scopes": ["orders:write"], "created_at": "2026-11-01T00:00:00Z", "extra": true});
        assert_eq!(parse_list(&json!([item.clone()])).len(), 1);
        assert_eq!(parse_list(&json!({"items": [item.clone()]}))[0].prefix, "sk_live_abcdefgh");
        assert_eq!(parse_list(&json!({"keys": [item]}))[0].id, 1);
        assert!(parse_list(&json!({"nope": 1})).is_empty());
        assert!(parse_list(&json!(null)).is_empty());
        // 잘못된 항목은 건너뛴다
        assert_eq!(parse_list(&json!([{"id": "x"}, {"id": 2}])).len(), 1);
    }

    #[test]
    fn created_shapes() {
        let secret = "sk_live_abcdefgh_0123456789012345678901234567890123456789";
        let flat = json!({"id": 3, "prefix": "sk_live_abcdefgh", "secret": secret});
        let (s, k) = parse_created(&flat).unwrap();
        assert_eq!((s.as_str(), k.id), (secret, 3));
        let nested = json!({"key": {"id": 4, "prefix": "sk_live_abcdefgh"}, "plaintext": secret});
        let (s, k) = parse_created(&nested).unwrap();
        assert_eq!((s.as_str(), k.id), (secret, 4));
        assert_eq!(parse_created(&json!({"id": 5, "prefix": "sk_live_abcdefgh"})), None);
    }

    #[test]
    fn masking() {
        assert_eq!(masked("sk_live_abcdefgh"), "sk_live_abcdefgh••••••••");
    }
}

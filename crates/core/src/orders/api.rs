//! S2S 주문 API의 HTTP 계약: 경로, 본문 한도, 전달 헤더, 인증 헤더 해석, 오류 코드.
//!
//! 엣지 Worker(`edge/worker`)와 원점(ingest `/v1/orders*`, `/internal/orders`)이 같은 값을 쓴다.
//! 그래서 같은 잘못된 요청은 어느 계층에서 거절되든 같은 상태 코드·`error` 코드를 받는다.

use super::{MAX_BATCH_BODY_BYTES, MAX_IDEMPOTENCY_KEY_LEN, MAX_SINGLE_BODY_BYTES};

/// 엣지 → 원점: 클라이언트의 `Authorization` 헤더 원문.
pub const HEADER_FORWARDED_AUTHORIZATION: &str = "x-forwarded-authorization";
/// 엣지 → 원점: 클라이언트가 호출한 공개 경로 (`/v1/orders`, `/v1/orders:batch`).
pub const HEADER_FORWARDED_PATH: &str = "x-forwarded-path";
/// 클라이언트 멱등 키 헤더 (엣지는 같은 이름으로 전달한다).
pub const HEADER_IDEMPOTENCY_KEY: &str = "idempotency-key";
/// `Authorization` 헤더 최대 길이 (`Bearer sk_live_...`는 100자 안쪽).
pub const MAX_AUTHORIZATION_LEN: usize = 512;

/// 주문 API 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrdersKind {
    Single,
    Batch,
}

impl OrdersKind {
    /// 클라이언트가 호출하는 공개 경로.
    pub const fn path(self) -> &'static str {
        match self {
            Self::Single => "/v1/orders",
            Self::Batch => "/v1/orders:batch",
        }
    }

    /// 본문 바이트 상한.
    pub const fn body_limit(self) -> usize {
        match self {
            Self::Single => MAX_SINGLE_BODY_BYTES,
            Self::Batch => MAX_BATCH_BODY_BYTES,
        }
    }

    /// 공개 경로 → 종류.
    pub fn from_path(path: &str) -> Option<Self> {
        [Self::Single, Self::Batch]
            .into_iter()
            .find(|k| k.path() == path)
    }
}

/// `Bearer <token>` 형식이면 토큰을 돌려준다. 스킴은 대소문자를 무시하고, 토큰은 비어 있거나
/// 공백을 포함할 수 없다.
pub fn bearer(v: &str) -> Option<&str> {
    let (scheme, token) = v.trim().split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer")
        && !token.is_empty()
        && !token.contains(char::is_whitespace))
    .then_some(token)
}

/// 멱등 키: 1~255자의 출력 가능한 ASCII (공백·제어 문자 불가).
pub fn valid_idempotency_key(k: &str) -> bool {
    (1..=MAX_IDEMPOTENCY_KEY_LEN).contains(&k.len()) && k.bytes().all(|b| b.is_ascii_graphic())
}

/// 주문 API 오류 (주문 내용 검증 오류 [`super::OrderError`]는 제외). 응답 본문은
/// `{"error": code, "message": message}`이고, 메시지는 정적 문자열이라 입력값을 반사하지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiError {
    /// 본문이 한도를 넘음
    TooLarge,
    /// `Authorization: Bearer ...` 헤더가 없거나 형식이 틀림
    MissingKey,
    /// 존재하지 않거나 폐기된 비밀 키 (엣지: 길이·문자 검사 실패)
    InvalidKey,
    InsufficientScope,
    ProjectInactive,
    /// JSON이 아님 (빈 본문 포함)
    BadJson,
    /// 일괄 본문이 `{"orders":[...]}`가 아님
    InvalidPayload,
    EmptyBatch,
    TooManyOrders,
    /// 같은 (이벤트 유형, 멱등 키)로 내용이 다른 요청이 이미 있음
    IdempotencyKeyReused,
    InvalidIdempotencyKey,
    /// 엣지 서명이 없거나 틀림 (`/internal/orders`)
    InvalidSignature,
    NotFound,
    StorageUnavailable,
    /// 엣지가 원점에 닿지 못함
    OriginUnavailable,
    Internal,
}

impl ApiError {
    pub const fn status(self) -> u16 {
        match self {
            Self::TooLarge | Self::TooManyOrders => 413,
            Self::MissingKey | Self::InvalidKey | Self::InvalidSignature => 401,
            Self::InsufficientScope | Self::ProjectInactive => 403,
            Self::BadJson | Self::InvalidPayload => 400,
            Self::EmptyBatch | Self::InvalidIdempotencyKey => 422,
            Self::IdempotencyKeyReused => 409,
            Self::NotFound => 404,
            Self::StorageUnavailable | Self::OriginUnavailable => 503,
            Self::Internal => 500,
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::MissingKey => "missing_key",
            Self::InvalidKey => "invalid_key",
            Self::InsufficientScope => "insufficient_scope",
            Self::ProjectInactive => "project_inactive",
            Self::BadJson => "bad_json",
            Self::InvalidPayload => "invalid_payload",
            Self::EmptyBatch => "empty_batch",
            Self::TooManyOrders => "too_many_orders",
            Self::IdempotencyKeyReused => "idempotency_key_reused",
            Self::InvalidIdempotencyKey => "invalid_idempotency_key",
            Self::InvalidSignature => "invalid_signature",
            Self::NotFound => "not_found",
            Self::StorageUnavailable => "storage_unavailable",
            Self::OriginUnavailable => "origin_unavailable",
            Self::Internal => "internal",
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::TooLarge => "본문이 너무 큽니다",
            Self::MissingKey => "Authorization: Bearer sk_... 헤더가 필요합니다",
            Self::InvalidKey => "유효하지 않은 비밀 키입니다",
            Self::InsufficientScope => "orders:write 권한이 없는 키입니다",
            Self::ProjectInactive => "비활성 프로젝트입니다",
            Self::BadJson => "JSON 형식이 아닙니다",
            Self::InvalidPayload => "{\"orders\": [...]} 형식이어야 합니다",
            Self::EmptyBatch => "주문이 없습니다",
            Self::TooManyOrders => "일괄 전송은 최대 500건입니다",
            Self::IdempotencyKeyReused => "같은 멱등 키로 내용이 다른 요청이 이미 처리되었습니다",
            Self::InvalidIdempotencyKey => "멱등 키는 1~255자의 출력 가능한 ASCII여야 합니다",
            Self::InvalidSignature => "엣지 서명이 없거나 올바르지 않습니다",
            Self::NotFound => "알 수 없는 경로",
            Self::StorageUnavailable => "일시적인 저장소 오류입니다. 같은 요청을 다시 보내 주세요",
            Self::OriginUnavailable => {
                "일시적으로 처리할 수 없습니다. 같은 요청을 다시 보내 주세요"
            }
            Self::Internal => "내부 오류입니다. 같은 요청을 다시 보내 주세요",
        }
    }

    /// 응답 본문 `{"error": code, "message": message}`.
    pub fn body(self) -> serde_json::Value {
        serde_json::json!({ "error": self.code(), "message": self.message() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_paths_and_limits() {
        assert_eq!(OrdersKind::Single.path(), "/v1/orders");
        assert_eq!(OrdersKind::Batch.path(), "/v1/orders:batch");
        assert_eq!(OrdersKind::Single.body_limit(), 64 * 1024);
        assert_eq!(OrdersKind::Batch.body_limit(), 1024 * 1024);
        assert_eq!(
            OrdersKind::from_path("/v1/orders"),
            Some(OrdersKind::Single)
        );
        assert_eq!(
            OrdersKind::from_path("/v1/orders:batch"),
            Some(OrdersKind::Batch)
        );
        assert_eq!(OrdersKind::from_path("/v1/orders:batchx"), None);
        assert_eq!(OrdersKind::from_path(""), None);
    }

    #[test]
    fn bearer_parsing() {
        assert_eq!(bearer("Bearer sk_live_a"), Some("sk_live_a"));
        assert_eq!(bearer("  bearer   sk_live_a  "), Some("sk_live_a"));
        assert_eq!(bearer("BEARER sk"), Some("sk"));
        for bad in [
            "",
            "   ",
            "Bearer",
            "Bearer ",
            "Basic abc",
            "Bearer a b",
            "Bearer\tx",
        ] {
            assert_eq!(bearer(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn idempotency_key_rules() {
        assert!(valid_idempotency_key("ORD-9901:confirm"));
        assert!(valid_idempotency_key(&"k".repeat(MAX_IDEMPOTENCY_KEY_LEN)));
        for bad in ["", "a b", "a\r\nb", "한글"] {
            assert!(!valid_idempotency_key(bad), "{bad:?}");
        }
        assert!(!valid_idempotency_key(
            &"k".repeat(MAX_IDEMPOTENCY_KEY_LEN + 1)
        ));
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(ApiError::TooLarge.status(), 413);
        assert_eq!(ApiError::MissingKey.code(), "missing_key");
        assert_eq!(ApiError::InvalidKey.status(), 401);
        let b = ApiError::InvalidPayload.body();
        assert_eq!(b["error"], "invalid_payload");
        assert_eq!(b["message"], "{\"orders\": [...]} 형식이어야 합니다");
    }
}

//! API 오류 → 화면에 보여줄 한국어 메시지.
//!
//! 서버 오류 본문은 `{"error": 코드, "message": 설명}`이다. 입력 검증(`invalid_input`)과
//! 충돌(`conflict`)은 서버 설명이 구체적이라 그대로 보여주고, 나머지는 코드별 고정 문구를 쓴다
//! (서버 내부 문구가 그대로 노출되지 않게).

use analytics_core::dto::admin::ApiErrorBody;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// 서버에 닿지 못함 (오프라인, CORS, DNS)
    Network,
    /// 세션 없음·만료 → 로그인 화면으로
    Unauthorized,
    InvalidCredentials,
    Forbidden,
    NotFound,
    /// 400 입력 오류 (서버 설명)
    Invalid(String),
    /// 409 (서버 설명)
    Conflict(String),
    Csrf,
    RateLimited,
    TooLarge,
    /// 5xx
    Server,
    /// 응답 JSON을 읽지 못함
    Decode,
}

impl ApiError {
    /// 상태 코드와 응답 본문으로 분류한다.
    pub fn from_response(status: u16, body: &str) -> ApiError {
        todo!()
    }

    pub fn message(&self) -> String {
        todo!()
    }

    /// 다시 시도하면 나아질 수 있는 오류인지 (재시도 버튼 노출).
    pub fn retryable(&self) -> bool {
        matches!(self, ApiError::Network | ApiError::Server | ApiError::RateLimited | ApiError::Decode)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

#[allow(dead_code)]
fn parse_body(body: &str) -> Option<ApiErrorBody> {
    serde_json::from_str(body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(code: &str, msg: &str) -> String {
        serde_json::json!({ "error": code, "message": msg }).to_string()
    }

    #[test]
    fn maps_status_and_code() {
        assert_eq!(ApiError::from_response(401, &body("unauthorized", "x")), ApiError::Unauthorized);
        assert_eq!(
            ApiError::from_response(401, &body("invalid_credentials", "x")),
            ApiError::InvalidCredentials
        );
        assert_eq!(ApiError::from_response(403, &body("forbidden", "x")), ApiError::Forbidden);
        assert_eq!(ApiError::from_response(403, &body("csrf_rejected", "x")), ApiError::Csrf);
        assert_eq!(ApiError::from_response(404, ""), ApiError::NotFound);
        assert_eq!(ApiError::from_response(429, &body("too_many_requests", "x")), ApiError::RateLimited);
        assert_eq!(ApiError::from_response(413, ""), ApiError::TooLarge);
        assert_eq!(ApiError::from_response(500, "<html>oops</html>"), ApiError::Server);
        assert_eq!(ApiError::from_response(502, ""), ApiError::Server);
    }

    #[test]
    fn invalid_input_keeps_server_message() {
        let e = ApiError::from_response(400, &body("invalid_input", "origin: 도메인만 입력하세요"));
        assert_eq!(e, ApiError::Invalid("origin: 도메인만 입력하세요".into()));
        assert_eq!(e.message(), "origin: 도메인만 입력하세요");
        let c = ApiError::from_response(409, &body("conflict", "이미 있는 이메일입니다"));
        assert_eq!(c.message(), "이미 있는 이메일입니다");
    }

    #[test]
    fn unparseable_or_empty_400_gets_generic_text() {
        assert_eq!(ApiError::from_response(400, "not json").message(), "입력값을 확인해 주세요");
        assert_eq!(ApiError::from_response(400, &body("invalid_input", "  ")).message(), "입력값을 확인해 주세요");
    }

    #[test]
    fn korean_messages() {
        assert_eq!(ApiError::Network.message(), "서버에 연결할 수 없습니다. 네트워크를 확인해 주세요");
        assert_eq!(ApiError::Unauthorized.message(), "로그인이 필요합니다");
        assert_eq!(ApiError::InvalidCredentials.message(), "이메일 또는 비밀번호가 올바르지 않습니다");
        assert_eq!(ApiError::Forbidden.message(), "이 작업을 할 권한이 없습니다");
        assert_eq!(ApiError::NotFound.message(), "요청한 데이터를 찾을 수 없습니다");
        assert_eq!(ApiError::RateLimited.message(), "요청이 너무 많습니다. 잠시 후 다시 시도해 주세요");
        assert_eq!(ApiError::Server.message(), "일시적인 서버 오류입니다. 잠시 후 다시 시도해 주세요");
        assert!(ApiError::Server.retryable() && !ApiError::Forbidden.retryable());
    }

    #[test]
    fn server_message_with_markup_is_kept_as_plain_text() {
        // 화면은 텍스트 노드로만 그리므로 그대로 두되, 아주 긴 설명은 자른다.
        let long = "가".repeat(500);
        let e = ApiError::from_response(400, &body("invalid_input", &long));
        assert_eq!(e.message().chars().count(), 201);
        assert!(e.message().ends_with('…'));
    }
}

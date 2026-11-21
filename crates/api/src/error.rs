//! API 오류. 본문은 항상 [`ApiErrorBody`] JSON이다.
//!
//! 내부 오류의 원인은 로그에만 남기고 응답에는 일반 문구만 넣는다.

use analytics_core::dto::admin::ApiErrorBody;
use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    retry_after: Option<u64>,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    /// 입력 검증 실패 (400 `invalid_input`)
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_input", message)
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "로그인이 필요합니다",
        )
    }

    /// 이메일이 없을 때와 비밀번호가 틀렸을 때 같은 응답 (사용자 열거 방지)
    pub fn invalid_credentials() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "이메일 또는 비밀번호가 올바르지 않습니다",
        )
    }

    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "권한이 없습니다")
    }

    /// 접근 권한이 없는 리소스도 404로 응답한다 (존재 여부 탐지 방지).
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "찾을 수 없습니다")
    }

    pub fn csrf() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "csrf_rejected",
            "허용되지 않은 요청 출처 또는 Content-Type",
        )
    }

    pub fn too_many_requests(retry_after_secs: u64) -> Self {
        let mut e = Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            "시도 횟수가 너무 많습니다. 잠시 후 다시 시도하세요",
        );
        e.retry_after = Some(retry_after_secs.max(1));
        e
    }

    pub fn payload_too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "요청 본문이 너무 큽니다",
        )
    }

    /// 리포트 저장소(ClickHouse) 장애. 원인은 로그에만 남긴다.
    pub fn storage_unavailable(err: anyhow::Error) -> Self {
        tracing::error!(error = ?err, "리포트 저장소 오류");
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "리포트 저장소에 일시적으로 연결할 수 없습니다. 잠시 후 다시 시도하세요",
        )
    }

    pub fn internal(err: anyhow::Error) -> Self {
        tracing::error!(error = ?err, "내부 오류");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "서버 오류가 발생했습니다",
        )
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorBody {
            error: self.code.to_owned(),
            message: self.message,
        };
        let mut res = (self.status, Json(body)).into_response();
        if let Some(v) = self
            .retry_after
            .and_then(|secs| HeaderValue::from_str(&secs.to_string()).ok())
        {
            res.headers_mut().insert(header::RETRY_AFTER, v);
        }
        res
    }
}

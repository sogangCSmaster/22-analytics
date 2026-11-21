//! 플랫폼 독립 HTTP 표현: 라우팅과 응답 구조체.
//!
//! `worker` 크레이트 타입은 wasm 계층(`glue`)에서만 쓴다. 여기서는 판단 결과를 평범한 값으로 만든다.

/// Worker가 돌려줄 응답. wasm 계층이 `worker::Response`로 옮긴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl EdgeResponse {
    pub fn empty(status: u16) -> Self {
        Self {
            status,
            headers: vec![],
            body: vec![],
        }
    }

    /// `{"error":"<code>"}` JSON 응답. code는 정적 문자열만 받는다 (입력값을 응답에 반사하지 않음).
    pub fn error(status: u16, code: &'static str) -> Self {
        Self {
            status,
            headers: vec![(
                "content-type".into(),
                "application/json; charset=utf-8".into(),
            )],
            body: format!(r#"{{"error":"{code}"}}"#).into_bytes(),
        }
    }

    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_owned(), value.into()));
        self
    }

    /// 대소문자 무시 헤더 조회 (테스트·로그용).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrdersKind {
    Single,
    Batch,
}

impl OrdersKind {
    /// 클라이언트가 호출한 경로. 원점에 `x-forwarded-path`로 전달한다.
    pub fn public_path(self) -> &'static str {
        match self {
            Self::Single => "/v1/orders",
            Self::Batch => "/v1/orders:batch",
        }
    }

    pub fn body_limit(self) -> usize {
        match self {
            Self::Single => crate::orders::MAX_ORDER_BYTES,
            Self::Batch => crate::orders::MAX_ORDER_BATCH_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Collect,
    Config,
    Orders(OrdersKind),
    /// 브라우저 CORS preflight (`/v1/collect`, `/v1/config`)
    Preflight,
    MethodNotAllowed,
    NotFound,
}

/// 메서드·경로로 처리기를 고른다. `/t.js` 등 정적 파일은 wrangler `[assets]`가 Worker 앞에서 처리한다.
pub fn route(method: &str, path: &str) -> Route {
    let _ = (method, path);
    todo!()
}

/// `Content-Length`가 한도를 넘는지 (본문을 읽기 전에 거른다).
pub fn declared_too_large(content_length: Option<&str>, limit: usize) -> bool {
    let _ = (content_length, limit);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_known_paths() {
        assert_eq!(route("POST", "/v1/collect"), Route::Collect);
        assert_eq!(route("GET", "/v1/config"), Route::Config);
        assert_eq!(
            route("POST", "/v1/orders"),
            Route::Orders(OrdersKind::Single)
        );
        assert_eq!(
            route("POST", "/v1/orders:batch"),
            Route::Orders(OrdersKind::Batch)
        );
        assert_eq!(route("OPTIONS", "/v1/collect"), Route::Preflight);
        assert_eq!(route("OPTIONS", "/v1/config"), Route::Preflight);
    }

    #[test]
    fn wrong_method_and_unknown_paths() {
        assert_eq!(route("GET", "/v1/collect"), Route::MethodNotAllowed);
        assert_eq!(route("POST", "/v1/config"), Route::MethodNotAllowed);
        assert_eq!(route("GET", "/v1/orders"), Route::MethodNotAllowed);
        // S2S 주문 API는 브라우저용이 아니므로 preflight를 받지 않는다.
        assert_eq!(route("OPTIONS", "/v1/orders"), Route::MethodNotAllowed);
        assert_eq!(route("POST", "/internal/collect"), Route::NotFound);
        assert_eq!(route("POST", "/v1/collect/"), Route::NotFound);
        assert_eq!(route("POST", "/v1/orders:batchx"), Route::NotFound);
        assert_eq!(route("GET", "/"), Route::NotFound);
    }

    #[test]
    fn declared_length_check() {
        assert!(!declared_too_large(None, 10));
        assert!(!declared_too_large(Some("10"), 10));
        assert!(declared_too_large(Some("11"), 10));
        assert!(declared_too_large(Some("99999999999999999999999"), 10));
        // 숫자가 아니면 본문을 실제로 읽어 다시 확인하므로 여기서는 통과시킨다.
        assert!(!declared_too_large(Some("abc"), 10));
    }

    #[test]
    fn error_body_never_reflects_input() {
        let r = EdgeResponse::error(401, "unauthorized");
        assert_eq!(r.body, br#"{"error":"unauthorized"}"#);
        assert_eq!(
            r.header("Content-Type"),
            Some("application/json; charset=utf-8")
        );
    }
}

//! API 클라이언트 (fetch, `credentials: include`).
//!
//! - 변경 요청에는 항상 `Content-Type: application/json`(광고비는 `text/csv`)을 붙인다 (CSRF 검사).
//! - 401은 로그인 화면으로 보낸다 (로그인 요청 자체는 제외).
//! - 경로·쿼리는 [`crate::logic::query`]에서 만든다.

use crate::logic::errors::ApiError;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// (상태 코드, 본문)
async fn raw(method: Method, path: &str, body: Option<(String, &'static str)>) -> ApiResult<(u16, String)> {
    let (status, text) = imp::fetch(method, path, body).await?;
    if status == 401 && path != LOGIN_PATH {
        crate::platform::redirect_to_login();
    }
    if (200..300).contains(&status) {
        Ok((status, text))
    } else {
        Err(ApiError::from_response(status, &text))
    }
}

pub const LOGIN_PATH: &str = "/v1/auth/login";

fn decode<T: DeserializeOwned>(text: &str) -> ApiResult<T> {
    serde_json::from_str(text).map_err(|_| ApiError::Decode)
}

pub async fn get<T: DeserializeOwned>(path: &str) -> ApiResult<T> {
    let (_, text) = raw(Method::Get, path, None).await?;
    decode(&text)
}

/// JSON 본문을 보내고 JSON 응답을 받는다.
pub async fn send<B: Serialize + ?Sized, T: DeserializeOwned>(
    method: Method,
    path: &str,
    body: &B,
) -> ApiResult<T> {
    let json = serde_json::to_string(body).map_err(|_| ApiError::Decode)?;
    let (_, text) = raw(method, path, Some((json, "application/json"))).await?;
    decode(&text)
}

/// 응답 본문이 없는 요청 (204). DELETE·로그아웃.
pub async fn send_empty(method: Method, path: &str) -> ApiResult<()> {
    raw(method, path, Some(("{}".into(), "application/json"))).await?;
    Ok(())
}

/// 광고비 CSV 업로드.
pub async fn post_csv<T: DeserializeOwned>(path: &str, csv: String) -> ApiResult<T> {
    let (_, text) = raw(Method::Post, path, Some((csv, "text/csv"))).await?;
    decode(&text)
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::{ApiResult, Method};
    use crate::logic::{errors::ApiError, query};
    use gloo_net::http::{Method as M, RequestBuilder};
    use web_sys::RequestCredentials;

    pub async fn fetch(
        method: Method,
        path: &str,
        body: Option<(String, &'static str)>,
    ) -> ApiResult<(u16, String)> {
        let url = query::join(query::api_base(), path);
        let m = match method {
            Method::Get => M::GET,
            Method::Post => M::POST,
            Method::Put => M::PUT,
            Method::Patch => M::PATCH,
            Method::Delete => M::DELETE,
        };
        let b = RequestBuilder::new(&url)
            .method(m)
            .credentials(RequestCredentials::Include)
            .header("Accept", "application/json");
        let req = match (method, body) {
            (Method::Get, _) | (_, None) => b.build(),
            (Method::Delete, Some((_, ct))) => b.header("Content-Type", ct).build(),
            (_, Some((text, ct))) => b.header("Content-Type", ct).body(text),
        }
        .map_err(|_| ApiError::Network)?;
        let res = req.send().await.map_err(|_| ApiError::Network)?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        Ok((status, text))
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::{ApiResult, Method};
    use crate::logic::errors::ApiError;

    pub async fn fetch(
        _method: Method,
        _path: &str,
        _body: Option<(String, &'static str)>,
    ) -> ApiResult<(u16, String)> {
        Err(ApiError::Network)
    }
}

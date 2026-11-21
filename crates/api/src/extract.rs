//! 요청 추출기: 인증 사용자, 프로젝트·광고주 접근 권한, 오류 형식을 맞춘 JSON·쿼리.
//!
//! 테넌트 격리 원칙: 접근할 수 없는 프로젝트·광고주는 존재하지 않는 것과 똑같이 404를 돌려준다.
//! 접근은 되지만 역할이 부족하면 403이다.

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use analytics_ch::ProjectScope;
use analytics_core::dto::admin::Role;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::{HeaderMap, StatusCode, header, request::Parts};
use serde::de::DeserializeOwned;
use std::collections::HashMap;

pub const SESSION_COOKIE: &str = "at_session";

// ---------------------------------------------------------------------------
// JSON·쿼리
// ---------------------------------------------------------------------------

/// `axum::Json`과 같지만 거부 응답을 [`ApiError`] 형식으로 바꾼다.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(v)) => Ok(Self(v)),
            Err(e) => Err(json_rejection(e)),
        }
    }
}

fn json_rejection(e: JsonRejection) -> ApiError {
    match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::payload_too_large(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Content-Type: application/json이 필요합니다",
        ),
        _ => ApiError::invalid(format!("JSON 본문 오류: {}", e.body_text())),
    }
}

/// `axum::extract::Query`와 같지만 거부 응답을 400 `invalid_input`으로 바꾼다.
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(v)) => Ok(Self(v)),
            Err(e) => Err(query_rejection(e)),
        }
    }
}

fn query_rejection(e: QueryRejection) -> ApiError {
    ApiError::invalid(format!("쿼리 파라미터 오류: {}", e.body_text()))
}

/// 경로 변수 하나를 숫자로 읽는다. 없거나 숫자가 아니면 404 (경로가 없는 것과 같게).
pub async fn path_id<T: std::str::FromStr, S: Send + Sync>(
    parts: &mut Parts,
    state: &S,
    name: &str,
) -> ApiResult<T> {
    let Path(params) = Path::<HashMap<String, String>>::from_request_parts(parts, state)
        .await
        .map_err(|_| ApiError::not_found())?;
    parse_id(params.get(name).map(String::as_str).unwrap_or(""))
}

/// 양수 id 파싱. 실패하면 404.
pub fn parse_id<T: std::str::FromStr>(s: &str) -> ApiResult<T> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError::not_found());
    }
    s.parse().map_err(|_| ApiError::not_found())
}

// ---------------------------------------------------------------------------
// 세션 쿠키
// ---------------------------------------------------------------------------

/// `Cookie` 헤더(여러 개 가능)에서 `at_session` 값을 찾는다.
pub fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == SESSION_COOKIE)
        .map(|(_, v)| v.trim().to_owned())
}

/// 로그인 시 `Set-Cookie`. 14일, HttpOnly, SameSite=Lax, Path=/ (+ Secure).
pub fn session_cookie(token: &str, secure: bool) -> String {
    let max_age = analytics_pg::sessions::TTL_DAYS * 24 * 60 * 60;
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure}")
}

/// 로그아웃 시 쿠키 삭제.
pub fn clear_session_cookie(secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{secure}")
}

/// 클라이언트 IP: `cf-connecting-ip` → `x-forwarded-for` 첫 값. 둘 다 없으면 `None`.
pub fn client_ip(headers: &HeaderMap) -> Option<String> {
    let clean = |s: &str| {
        let s = s.trim();
        s.parse::<std::net::IpAddr>().ok().map(|ip| ip.to_string())
    };
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(clean)
        .or_else(|| {
            headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .and_then(clean)
        })
}

// ---------------------------------------------------------------------------
// 인증 사용자
// ---------------------------------------------------------------------------

/// 유효한 세션의 사용자. 없으면 401.
#[derive(Debug, Clone)]
pub struct AuthUser(pub analytics_pg::users::User);

impl AuthUser {
    pub fn id(&self) -> i64 {
        self.0.id
    }

    pub fn is_super_admin(&self) -> bool {
        self.0.is_super_admin
    }

    /// 슈퍼 관리자가 아니면 403.
    pub fn require_super_admin(&self) -> ApiResult<()> {
        if self.is_super_admin() {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        if let Some(u) = parts.extensions.get::<AuthUser>() {
            return Ok(u.clone());
        }
        let token = session_token(&parts.headers).ok_or_else(ApiError::unauthorized)?;
        let user = analytics_pg::sessions::lookup(&state.pg, &token)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
        let u = AuthUser(user);
        parts.extensions.insert(u.clone());
        Ok(u)
    }
}

fn role_from_db(s: &str) -> ApiResult<Role> {
    Role::parse(s).ok_or_else(|| ApiError::internal(anyhow::anyhow!("알 수 없는 역할: {s}")))
}

/// 역할이 부족하면 403.
pub fn require_role(have: Role, min: Role) -> ApiResult<()> {
    if have >= min {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

// ---------------------------------------------------------------------------
// 프로젝트 접근
// ---------------------------------------------------------------------------

/// 경로 `{project_id}`의 프로젝트에 접근 권한이 확인된 상태.
/// 슈퍼 관리자는 모든 프로젝트에 `admin`으로 접근한다.
#[derive(Debug, Clone)]
pub struct ProjectAccess {
    pub user: AuthUser,
    pub scope: ProjectScope,
    pub role: Role,
    pub project: analytics_pg::projects::Project,
}

impl ProjectAccess {
    pub fn id(&self) -> i32 {
        self.project.id
    }

    pub fn require(&self, min: Role) -> ApiResult<()> {
        require_role(self.role, min)
    }
}

impl FromRequestParts<AppState> for ProjectAccess {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        let id: i32 = path_id(parts, state, "project_id").await?;
        let (project, role) = if user.is_super_admin() {
            let p = analytics_pg::projects::get(&state.pg, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            (p, Role::Admin)
        } else {
            let pr = analytics_pg::projects::get_for_user(&state.pg, user.id(), id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let role = role_from_db(&pr.role)?;
            (pr.project, role)
        };
        let project_id = u32::try_from(project.id).map_err(|_| ApiError::not_found())?;
        Ok(Self {
            user,
            scope: ProjectScope::authorized(project_id),
            role,
            project,
        })
    }
}

// ---------------------------------------------------------------------------
// 광고주 접근
// ---------------------------------------------------------------------------

/// 경로 `{org_id}`의 광고주에 접근 권한이 확인된 상태.
#[derive(Debug, Clone)]
pub struct OrgAccess {
    pub user: AuthUser,
    pub role: Role,
    pub org: analytics_pg::orgs::Org,
}

impl OrgAccess {
    pub fn id(&self) -> i64 {
        self.org.id
    }

    pub fn require(&self, min: Role) -> ApiResult<()> {
        require_role(self.role, min)
    }
}

impl FromRequestParts<AppState> for OrgAccess {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        let id: i64 = path_id(parts, state, "org_id").await?;
        let (org, role) = if user.is_super_admin() {
            let o = analytics_pg::orgs::get(&state.pg, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            (o, Role::Admin)
        } else {
            let o = analytics_pg::orgs::get_for_user(&state.pg, user.id(), id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let role = role_from_db(&o.role)?;
            (
                analytics_pg::orgs::Org {
                    id: o.id,
                    name: o.name,
                    memo: o.memo,
                    created_at: o.created_at,
                },
                role,
            )
        };
        Ok(Self { user, role, org })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn finds_session_cookie_among_others() {
        let mut h = HeaderMap::new();
        h.append(
            header::COOKIE,
            HeaderValue::from_static("a=1; at_session=abc ; b=2"),
        );
        assert_eq!(session_token(&h).as_deref(), Some("abc"));
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, HeaderValue::from_static("x=1"));
        h.append(header::COOKIE, HeaderValue::from_static("at_session=zz"));
        assert_eq!(session_token(&h).as_deref(), Some("zz"));
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, HeaderValue::from_static("xat_session=1"));
        assert_eq!(session_token(&h), None);
    }

    #[test]
    fn client_ip_prefers_cf_header_and_first_xff_hop() {
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static(" 10.0.0.1 , 172.16.0.1"),
        );
        assert_eq!(client_ip(&h).as_deref(), Some("10.0.0.1"));
        h.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.5"));
        assert_eq!(client_ip(&h).as_deref(), Some("203.0.113.5"));
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("not-an-ip"));
        assert_eq!(client_ip(&h), None);
    }

    #[test]
    fn ids_must_be_plain_digits() {
        assert_eq!(parse_id::<i32>("12").unwrap(), 12);
        for bad in ["", "-1", "+1", "1e3", "abc", "99999999999"] {
            assert!(parse_id::<i32>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn cookie_attributes() {
        let c = session_cookie("tok", true);
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Lax") && c.ends_with("Secure"));
        assert!(!session_cookie("tok", false).contains("Secure"));
        assert!(clear_session_cookie(false).contains("Max-Age=0"));
    }
}

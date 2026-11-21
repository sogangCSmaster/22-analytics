//! 요청 추출기: 인증 사용자, 프로젝트·광고주 접근 권한, 오류 형식을 맞춘 JSON·쿼리.
//!
//! 테넌트 격리 원칙: 접근할 수 없는 프로젝트·광고주는 존재하지 않는 것과 똑같이 404를 돌려준다.
//! 접근은 되지만 역할이 부족하면 403이다.

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use analytics_ch::ProjectScope;
use analytics_core::dto::admin::Role;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts};
use axum::middleware::Next;
use axum::response::Response;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

/// 세션 쿠키 이름 (`COOKIE_SECURE=false`, http 개발용).
pub const SESSION_COOKIE: &str = "at_session";
/// 세션 쿠키 이름 (`COOKIE_SECURE=true`).
pub const SECURE_SESSION_COOKIE: &str = "__Host-at_session";

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

/// 모드별 세션 쿠키 이름. `secure`면 `__Host-` 접두사: 브라우저가 Secure·`Path=/`·Domain 없음을
/// 강제하므로 하위 도메인이나 평문 http 응답이 같은 이름의 쿠키를 심을 수 없다.
pub fn session_cookie_name(secure: bool) -> &'static str {
    if secure {
        SECURE_SESSION_COOKIE
    } else {
        SESSION_COOKIE
    }
}

/// `Cookie` 헤더(여러 개 가능)에서 현재 모드의 세션 쿠키 값을 찾는다.
///
/// 같은 이름이 서로 다른 값으로 두 번 이상 오면 `None`: 공격자가 심은 쿠키와 진짜 쿠키 중
/// 어느 쪽이 먼저 오는지는 브라우저 정렬 규칙에 달려 있어 믿을 수 없다. 같은 값의 반복은 허용.
pub fn session_token(headers: &HeaderMap, secure: bool) -> Option<String> {
    let name = session_cookie_name(secure);
    let mut found: Option<&str> = None;
    for (k, v) in headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
    {
        if k.trim() != name {
            continue;
        }
        let v = v.trim();
        match found {
            None => found = Some(v),
            Some(prev) if prev == v => {}
            Some(_) => return None,
        }
    }
    found.map(str::to_owned)
}

/// 로그인·세션 연장 시 `Set-Cookie`. 14일, HttpOnly, SameSite=Lax, Path=/ (+ Secure).
pub fn session_cookie(token: &str, secure: bool) -> String {
    let name = session_cookie_name(secure);
    let max_age = analytics_pg::sessions::TTL_DAYS * 24 * 60 * 60;
    let secure = if secure { "; Secure" } else { "" };
    format!("{name}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure}")
}

/// 로그아웃 시 쿠키 삭제.
pub fn clear_session_cookie(secure: bool) -> String {
    let name = session_cookie_name(secure);
    let secure = if secure { "; Secure" } else { "" };
    format!("{name}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{secure}")
}

/// 세션이 연장됐을 때 응답에 쿠키를 다시 실어 보내기 위한 칸.
/// [`session_refresh`] 미들웨어가 요청마다 넣고, [`AuthUser`] 추출기가 채운다.
#[derive(Clone, Default)]
pub struct SessionRefresh(Arc<Mutex<Option<String>>>);

impl SessionRefresh {
    fn set(&self, token: String) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(token);
    }

    fn take(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// 세션 슬라이딩 연장을 쿠키에도 반영한다. DB 만료만 늘리면 브라우저 쿠키(Max-Age)는
/// 로그인 14일 뒤에 사라져 활발히 쓰는 사용자도 로그아웃된다.
/// 핸들러가 이미 세션 쿠키를 정했으면(로그인·로그아웃) 건드리지 않는다.
pub async fn session_refresh(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let slot = SessionRefresh::default();
    req.extensions_mut().insert(slot.clone());
    let mut res = next.run(req).await;
    if let Some(token) = slot.take()
        && !res.headers().contains_key(header::SET_COOKIE)
        && let Ok(v) = HeaderValue::from_str(&session_cookie(&token, state.config.cookie_secure))
    {
        res.headers_mut().append(header::SET_COOKIE, v);
    }
    res
}

// ---------------------------------------------------------------------------
// 클라이언트 IP
// ---------------------------------------------------------------------------

/// 요청의 클라이언트 IP와 그 출처.
///
/// - `peer`: 소켓 상대 주소 (`ConnectInfo`). 위조할 수 없다.
/// - `supplied`: `TRUST_PROXY_HEADERS=true`일 때만 읽는 `cf-connecting-ip` → `x-forwarded-for`
///   첫 값. 프록시 뒤가 아니면 누구나 써 넣을 수 있으므로 기본은 무시한다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientIp {
    pub peer: Option<IpAddr>,
    pub supplied: Option<IpAddr>,
}

impl ClientIp {
    /// rate limit 등에 쓰는 IP: 믿을 수 있는 헤더 값 → 소켓 주소.
    pub fn ip(&self) -> Option<IpAddr> {
        self.supplied.or(self.peer)
    }

    /// 감사 로그용: 출처를 구분해 남긴다 (헤더 값을 검증된 IP처럼 기록하지 않는다).
    pub fn audit_fields(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        if let Some(ip) = self.supplied {
            m.insert("client_supplied_ip".into(), ip.to_string().into());
        }
        if let Some(ip) = self.peer {
            m.insert("peer_ip".into(), ip.to_string().into());
        }
        serde_json::Value::Object(m)
    }
}

/// `trust_proxy_headers`가 false면 헤더를 보지 않는다.
pub fn client_ip(headers: &HeaderMap, peer: Option<IpAddr>, trust_proxy_headers: bool) -> ClientIp {
    let supplied = if trust_proxy_headers {
        let parse = |s: &str| s.trim().parse::<IpAddr>().ok();
        headers
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(parse)
            .or_else(|| {
                headers
                    .get("x-forwarded-for")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.split(',').next())
                    .and_then(parse)
            })
    } else {
        None
    };
    ClientIp { peer, supplied }
}

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        Ok(client_ip(
            &parts.headers,
            peer,
            state.config.trust_proxy_headers,
        ))
    }
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
        let token = session_token(&parts.headers, state.config.cookie_secure)
            .ok_or_else(ApiError::unauthorized)?;
        let (user, refreshed) = analytics_pg::sessions::lookup_refreshing(&state.pg, &token)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
        if refreshed && let Some(slot) = parts.extensions.get::<SessionRefresh>() {
            slot.set(token);
        }
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
        assert_eq!(session_token(&h, false).as_deref(), Some("abc"));
        assert_eq!(
            session_token(&h, true),
            None,
            "Secure 모드는 __Host- 이름만"
        );
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, HeaderValue::from_static("x=1"));
        h.append(header::COOKIE, HeaderValue::from_static("at_session=zz"));
        assert_eq!(session_token(&h, false).as_deref(), Some("zz"));
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, HeaderValue::from_static("xat_session=1"));
        assert_eq!(session_token(&h, false), None);
        let mut h = HeaderMap::new();
        h.append(
            header::COOKIE,
            HeaderValue::from_static("at_session=1; __Host-at_session=2"),
        );
        assert_eq!(session_token(&h, true).as_deref(), Some("2"));
        assert_eq!(session_token(&h, false).as_deref(), Some("1"));
    }

    #[test]
    fn duplicate_session_cookies_with_different_values_are_ignored() {
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, HeaderValue::from_static("at_session=a"));
        h.append(header::COOKIE, HeaderValue::from_static("at_session=b"));
        assert_eq!(session_token(&h, false), None);
        let mut h = HeaderMap::new();
        h.append(
            header::COOKIE,
            HeaderValue::from_static("at_session=a; at_session=a"),
        );
        assert_eq!(session_token(&h, false).as_deref(), Some("a"));
    }

    #[test]
    fn client_ip_reads_headers_only_when_trusted() {
        let peer: Option<IpAddr> = Some("192.0.2.1".parse().unwrap());
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static(" 10.0.0.1 , 172.16.0.1"),
        );
        assert_eq!(client_ip(&h, peer, true).ip(), "10.0.0.1".parse().ok());
        assert_eq!(client_ip(&h, peer, false).ip(), peer, "기본은 헤더 무시");
        h.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.5"));
        assert_eq!(client_ip(&h, peer, true).ip(), "203.0.113.5".parse().ok());
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("not-an-ip"));
        assert_eq!(client_ip(&h, peer, true).ip(), peer);
        assert_eq!(client_ip(&h, None, true).ip(), None);
    }

    #[test]
    fn audit_fields_label_the_ip_source() {
        let c = ClientIp {
            peer: "192.0.2.1".parse().ok(),
            supplied: "203.0.113.5".parse().ok(),
        };
        assert_eq!(
            c.audit_fields(),
            serde_json::json!({ "client_supplied_ip": "203.0.113.5", "peer_ip": "192.0.2.1" })
        );
        assert_eq!(ClientIp::default().audit_fields(), serde_json::json!({}));
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
        assert!(c.starts_with("__Host-at_session=tok;") && !c.contains("Domain"));
        assert!(!session_cookie("tok", false).contains("Secure"));
        assert!(session_cookie("tok", false).starts_with("at_session=tok;"));
        assert!(clear_session_cookie(false).contains("Max-Age=0"));
        assert!(clear_session_cookie(true).starts_with("__Host-at_session=;"));
    }
}

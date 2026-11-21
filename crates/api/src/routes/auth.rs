//! `POST /v1/auth/login`, `POST /v1/auth/logout`, `GET /v1/auth/me`.

use super::{audit, project_dto, user_dto};
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{
    ApiJson, AuthUser, ClientIp, clear_session_cookie, session_cookie, session_token,
};
use crate::{ratelimit, validate};
use analytics_core::dto::admin::{LoginRequest, MeResponse, OrgAccess, ProjectSummary, Role};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn login(
    State(state): State<AppState>,
    client: ClientIp,
    headers: HeaderMap,
    ApiJson(req): ApiJson<LoginRequest>,
) -> ApiResult<Response> {
    let email = validate::normalize_email(&req.email);
    // 길이 초과 입력은 제한기·DB·argon2를 거치지 않고 일반 실패와 같은 응답으로 끝낸다
    // (제한기 키를 늘리거나 큰 입력으로 해시 비용을 쓰게 하지 못하게). 길이는 비밀이 아니다.
    if email.len() > validate::MAX_EMAIL_BYTES
        || req.password.chars().count() > validate::MAX_PASSWORD_CHARS
    {
        return Err(ApiError::invalid_credentials());
    }
    let ip = client.ip();
    let email_key = ratelimit::email_hash(&email);
    // 로그에는 이메일 원문 대신 해시 앞부분만 남긴다 (같은 계정의 시도끼리 묶을 수 있을 만큼).
    let email_tag = hex::encode(&email_key[..6]);

    // 상한 검사와 "진행 중" 기록을 한 번에: 동시에 보낸 시도도 상한을 넘지 못한다.
    let reservation = match state.limiter.reserve(&email_key, ip) {
        Ok(r) => r,
        Err(retry_after) => {
            tracing::warn!(email = %email_tag, ?ip, "로그인 rate limit");
            return Err(ApiError::too_many_requests(retry_after));
        }
    };

    // 없는 이메일도 같은 비용의 argon2 검증을 거친다 (응답 시간으로 계정 존재를 알 수 없게).
    let creds = analytics_pg::users::credentials_by_email(&state.pg, &email).await?;
    let password = req.password;
    let user_id = tokio::task::spawn_blocking(move || match creds {
        Some((id, hash)) => analytics_pg::users::verify_password(&password, &hash).then_some(id),
        None => {
            analytics_pg::users::dummy_verify(&password);
            None
        }
    })
    .await
    .map_err(|e| ApiError::internal(e.into()))?;

    let Some(user_id) = user_id else {
        reservation.fail();
        tracing::info!(email = %email_tag, ?ip, "로그인 실패");
        return Err(ApiError::invalid_credentials());
    };
    reservation.succeed();

    // 세션 고정 방지: 요청에 실려 온 기존 세션은 지우고 항상 새 토큰을 발급한다.
    if let Some(old) = session_token(&headers, state.config.cookie_secure) {
        analytics_pg::sessions::delete(&state.pg, &old).await?;
    }
    let token = analytics_pg::sessions::create(&state.pg, user_id).await?;
    analytics_pg::users::touch_login(&state.pg, user_id).await?;
    let user = analytics_pg::users::by_id(&state.pg, user_id)
        .await?
        .ok_or_else(ApiError::invalid_credentials)?;
    audit(
        &state,
        None,
        user_id,
        "auth.login",
        format!("user:{user_id}"),
        client.audit_fields(),
    )
    .await?;

    let me = me_response(&state, &AuthUser(user)).await?;
    let cookie = session_cookie(&token, state.config.cookie_secure);
    Ok(([(header::SET_COOKIE, cookie)], Json(me)).into_response())
}

/// 로그아웃. 세션이 없거나 이미 지워졌어도 204 (멱등).
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let token = session_token(&headers, state.config.cookie_secure);
    let user = match token.as_deref() {
        Some(t) => analytics_pg::sessions::lookup(&state.pg, t).await?,
        None => None,
    };
    if let (Some(token), Some(user)) = (token, user) {
        analytics_pg::sessions::delete(&state.pg, &token).await?;
        audit(
            &state,
            None,
            user.id,
            "auth.logout",
            format!("user:{}", user.id),
            json!({}),
        )
        .await?;
    }
    let cookie = clear_session_cookie(state.config.cookie_secure);
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response())
}

pub async fn me(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<MeResponse>> {
    Ok(Json(me_response(&state, &user).await?))
}

async fn me_response(state: &AppState, user: &AuthUser) -> ApiResult<MeResponse> {
    let orgs = if user.is_super_admin() {
        analytics_pg::orgs::list_all(&state.pg).await?
    } else {
        analytics_pg::orgs::list_for_user(&state.pg, user.id()).await?
    };
    let ids: Vec<i64> = orgs.iter().map(|o| o.id).collect();
    let projects = analytics_pg::projects::list_by_orgs(&state.pg, &ids).await?;
    let orgs = orgs
        .into_iter()
        .map(|o| {
            let role = Role::parse(&o.role).unwrap_or(Role::Viewer);
            OrgAccess {
                id: o.id,
                name: o.name,
                role,
                projects: projects
                    .iter()
                    .filter(|p| p.org_id == o.id)
                    .map(|p| {
                        let d = project_dto(p, role);
                        ProjectSummary {
                            id: d.id,
                            name: d.name,
                            status: d.status,
                        }
                    })
                    .collect(),
            }
        })
        .collect();
    Ok(MeResponse {
        user: user_dto(&user.0),
        orgs,
    })
}

//! `POST /v1/users`, `GET /v1/users` (슈퍼 관리자 전용).

use super::{audit, user_dto};
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, AuthUser};
use crate::validate;
use analytics_core::dto::admin::{CreateUserRequest, Page, UserDto};
use analytics_pg::users::{CreateUserError, NewUser};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;

pub const DEFAULT_LIMIT: i64 = 50;
pub const MAX_LIMIT: i64 = 200;
pub const MAX_OFFSET: i64 = 1_000_000;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub q: Option<String>,
}

/// 페이지 파라미터 검증: limit 1..=200 (기본 50), offset 0..=1,000,000.
pub fn page_params(limit: Option<i64>, offset: Option<i64>) -> ApiResult<(i64, i64)> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let offset = offset.unwrap_or(0);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(ApiError::invalid(format!("limit: 1~{MAX_LIMIT}")));
    }
    if !(0..=MAX_OFFSET).contains(&offset) {
        return Err(ApiError::invalid(format!("offset: 0~{MAX_OFFSET}")));
    }
    Ok((limit, offset))
}

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(req): ApiJson<CreateUserRequest>,
) -> ApiResult<(StatusCode, Json<UserDto>)> {
    user.require_super_admin()?;
    let email = validate::email(&req.email)?;
    validate::password(&req.password)?;
    let name = validate::optional_name("name", &req.name, validate::MAX_NAME_CHARS)?;

    let password = req.password;
    let password_hash =
        tokio::task::spawn_blocking(move || analytics_pg::users::hash_password(&password))
            .await
            .map_err(|e| ApiError::internal(e.into()))??;
    let created = analytics_pg::users::create(
        &state.pg,
        &NewUser {
            email,
            password_hash,
            name,
            is_super_admin: req.is_super_admin,
        },
    )
    .await
    .map_err(|e| match e {
        CreateUserError::EmailTaken => ApiError::new(
            StatusCode::CONFLICT,
            "email_taken",
            "이미 사용 중인 이메일입니다",
        ),
        CreateUserError::Other(e) => ApiError::internal(e),
    })?;
    audit(
        &state.pg,
        None,
        user.id(),
        "user.create",
        format!("user:{}", created.id),
        json!({ "email": created.email, "is_super_admin": created.is_super_admin }),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(user_dto(&created))))
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Page<UserDto>>> {
    user.require_super_admin()?;
    let (limit, offset) = page_params(q.limit, q.offset)?;
    let search = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if search.is_some_and(|s| s.chars().count() > validate::MAX_NAME_CHARS) {
        return Err(ApiError::invalid("q: 검색어가 너무 깁니다"));
    }
    let (rows, total) = analytics_pg::users::list(&state.pg, search, limit, offset).await?;
    Ok(Json(Page {
        items: rows.iter().map(user_dto).collect(),
        total,
        limit,
        offset,
    }))
}

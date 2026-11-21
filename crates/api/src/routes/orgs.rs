//! 광고주와 멤버.
//!
//! - `GET /v1/orgs`: 접근 가능한 광고주 (슈퍼 관리자는 전체)
//! - `POST /v1/orgs`: 슈퍼 관리자
//! - `GET /v1/orgs/{org_id}`: 멤버
//! - `PATCH /v1/orgs/{org_id}`: 광고주 admin
//! - `GET/POST /v1/orgs/{org_id}/members`, `DELETE /v1/orgs/{org_id}/members/{user_id}`: 광고주 admin

use super::{audit, org_dto};
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, AuthUser, OrgAccess, parse_id};
use crate::validate;
use analytics_core::dto::admin::{
    AddMemberRequest, CreateOrgRequest, MemberDto, OrgDto, Role, UpdateOrgRequest,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::json;
use std::collections::HashMap;

pub async fn list(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<OrgDto>>> {
    let rows = if user.is_super_admin() {
        analytics_pg::orgs::list_all(&state.pg).await?
    } else {
        analytics_pg::orgs::list_for_user(&state.pg, user.id()).await?
    };
    Ok(Json(
        rows.into_iter()
            .map(|o| OrgDto {
                id: o.id,
                created_at: super::ts(o.created_at),
                role: Role::parse(&o.role).unwrap_or(Role::Viewer),
                name: o.name,
                memo: o.memo,
            })
            .collect(),
    ))
}

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(req): ApiJson<CreateOrgRequest>,
) -> ApiResult<(StatusCode, Json<OrgDto>)> {
    user.require_super_admin()?;
    let name = validate::name("name", &req.name, validate::MAX_NAME_CHARS)?;
    let memo = validate::memo(&req.memo)?;
    let org = analytics_pg::orgs::create(&state.pg, &name, &memo).await?;
    audit(
        &state,
        Some(org.id),
        user.id(),
        "org.create",
        format!("org:{}", org.id),
        json!({ "name": org.name }),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(org_dto(&org, Role::Admin))))
}

pub async fn get(access: OrgAccess) -> ApiResult<Json<OrgDto>> {
    Ok(Json(org_dto(&access.org, access.role)))
}

pub async fn update(
    State(state): State<AppState>,
    access: OrgAccess,
    ApiJson(req): ApiJson<UpdateOrgRequest>,
) -> ApiResult<Json<OrgDto>> {
    access.require(Role::Admin)?;
    let name = req
        .name
        .as_deref()
        .map(|n| validate::name("name", n, validate::MAX_NAME_CHARS))
        .transpose()?;
    let memo = req.memo.as_deref().map(validate::memo).transpose()?;
    let org = analytics_pg::orgs::update(&state.pg, access.id(), name.as_deref(), memo.as_deref())
        .await?
        .ok_or_else(ApiError::not_found)?;
    audit(
        &state,
        Some(org.id),
        access.user.id(),
        "org.update",
        format!("org:{}", org.id),
        json!({ "name": name, "memo_changed": memo.is_some() }),
    )
    .await?;
    Ok(Json(org_dto(&org, access.role)))
}

fn member_dto(m: analytics_pg::orgs::Member) -> MemberDto {
    MemberDto {
        user_id: m.user_id,
        email: m.email,
        name: m.name,
        role: Role::parse(&m.role).unwrap_or(Role::Viewer),
    }
}

pub async fn members(
    State(state): State<AppState>,
    access: OrgAccess,
) -> ApiResult<Json<Vec<MemberDto>>> {
    access.require(Role::Admin)?;
    let rows = analytics_pg::orgs::members(&state.pg, access.id()).await?;
    Ok(Json(rows.into_iter().map(member_dto).collect()))
}

/// 멤버 추가 또는 역할 변경 (멱등). 계정은 슈퍼 관리자가 미리 만들어 둔다.
pub async fn add_member(
    State(state): State<AppState>,
    access: OrgAccess,
    ApiJson(req): ApiJson<AddMemberRequest>,
) -> ApiResult<Json<MemberDto>> {
    access.require(Role::Admin)?;
    let email = validate::normalize_email(&req.email);
    let Some(target) = analytics_pg::users::by_email(&state.pg, &email).await? else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "user_not_found",
            "해당 이메일의 계정이 없습니다. 먼저 계정을 만드세요",
        ));
    };
    analytics_pg::orgs::upsert_member(&state.pg, access.id(), target.id, req.role.as_str()).await?;
    audit(
        &state,
        Some(access.id()),
        access.user.id(),
        "member.upsert",
        format!("user:{}", target.id),
        json!({ "role": req.role }),
    )
    .await?;
    Ok(Json(MemberDto {
        user_id: target.id,
        email: target.email,
        name: target.name,
        role: req.role,
    }))
}

/// 멤버 제거. 이미 없어도 204 (멱등). 감사 로그는 실제로 지운 경우에만 남긴다.
pub async fn remove_member(
    State(state): State<AppState>,
    access: OrgAccess,
    Path(params): Path<HashMap<String, String>>,
) -> ApiResult<StatusCode> {
    access.require(Role::Admin)?;
    let user_id: i64 = parse_id(params.get("user_id").map(String::as_str).unwrap_or(""))?;
    if analytics_pg::orgs::remove_member(&state.pg, access.id(), user_id).await? {
        audit(
            &state,
            Some(access.id()),
            access.user.id(),
            "member.remove",
            format!("user:{user_id}"),
            json!({}),
        )
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

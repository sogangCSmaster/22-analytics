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
        &state.pg,
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
        &state.pg,
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

fn last_admin() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "last_admin",
        "광고주에는 admin이 한 명 이상 있어야 합니다. 다른 admin을 먼저 지정하세요",
    )
}

/// `user_id`가 이 광고주에서 admin 권한을 잃으면(강등·제거) admin이 없어지는지.
/// 호출자는 [`analytics_pg::orgs::lock`]으로 광고주를 잠근 트랜잭션 안에서 부른다.
/// 슈퍼 관리자는 멤버십 없이도 접근하므로 "남은 admin"으로 세지 않는다.
async fn would_remove_last_admin(
    conn: &mut sqlx::PgConnection,
    org_id: i64,
    user_id: i64,
) -> ApiResult<bool> {
    let Some((role, is_super_admin)) =
        analytics_pg::orgs::member_role(&mut *conn, org_id, user_id).await?
    else {
        return Ok(false);
    };
    if role != Role::Admin.as_str() || is_super_admin {
        return Ok(false);
    }
    Ok(analytics_pg::orgs::count_member_admins(&mut *conn, org_id).await? <= 1)
}

/// 멤버 추가 또는 역할 변경 (멱등). 계정은 슈퍼 관리자가 미리 만들어 둔다.
///
/// 광고주 admin(슈퍼 관리자 아님)은 테넌트 경계를 넘지 못한다: 이미 이 광고주의 멤버이거나,
/// 어느 광고주에도 속하지 않은 일반 계정만 다룰 수 있다. 없는 계정·슈퍼 관리자·다른 광고주
/// 소속 계정은 모두 같은 403이라, 이메일로 계정 존재나 소속을 알아낼 수 없다.
///
/// 마지막 admin을 강등하면 409 `last_admin`.
pub async fn add_member(
    State(state): State<AppState>,
    access: OrgAccess,
    ApiJson(req): ApiJson<AddMemberRequest>,
) -> ApiResult<Json<MemberDto>> {
    access.require(Role::Admin)?;
    let email = validate::normalize_email(&req.email);
    let target = analytics_pg::users::by_email(&state.pg, &email).await?;
    let target = if access.user.is_super_admin() {
        target.ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "user_not_found",
                "해당 이메일의 계정이 없습니다. 먼저 계정을 만드세요",
            )
        })?
    } else {
        let Some(target) = target.filter(|t| !t.is_super_admin) else {
            return Err(ApiError::forbidden());
        };
        // 이미 이 광고주 멤버(목록에서 보이는 사람)면 역할 변경 허용, 아니면 무소속 계정만.
        let orgs = analytics_pg::orgs::membership_org_ids(&state.pg, target.id).await?;
        if !orgs.is_empty() && !orgs.contains(&access.id()) {
            return Err(ApiError::forbidden());
        }
        target
    };

    let mut tx = state.pg.begin().await?;
    if !analytics_pg::orgs::lock(&mut tx, access.id()).await? {
        return Err(ApiError::not_found());
    }
    if req.role != Role::Admin && would_remove_last_admin(&mut tx, access.id(), target.id).await? {
        return Err(last_admin());
    }
    analytics_pg::orgs::upsert_member(&mut *tx, access.id(), target.id, req.role.as_str()).await?;
    audit(
        &mut *tx,
        Some(access.id()),
        access.user.id(),
        "member.upsert",
        format!("user:{}", target.id),
        json!({ "role": req.role }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(MemberDto {
        user_id: target.id,
        email: target.email,
        name: target.name,
        role: req.role,
    }))
}

/// 멤버 제거. 이미 없어도 204 (멱등). 감사 로그는 실제로 지운 경우에만 남긴다.
/// 마지막 admin이면 409 `last_admin`.
pub async fn remove_member(
    State(state): State<AppState>,
    access: OrgAccess,
    Path(params): Path<HashMap<String, String>>,
) -> ApiResult<StatusCode> {
    access.require(Role::Admin)?;
    let user_id: i64 = parse_id(params.get("user_id").map(String::as_str).unwrap_or(""))?;
    let mut tx = state.pg.begin().await?;
    if !analytics_pg::orgs::lock(&mut tx, access.id()).await? {
        return Err(ApiError::not_found());
    }
    if would_remove_last_admin(&mut tx, access.id(), user_id).await? {
        return Err(last_admin());
    }
    if analytics_pg::orgs::remove_member(&mut *tx, access.id(), user_id).await? {
        audit(
            &mut *tx,
            Some(access.id()),
            access.user.id(),
            "member.remove",
            format!("user:{user_id}"),
            json!({}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

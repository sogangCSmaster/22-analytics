//! S2S 주문 API 비밀 키(`sk_live_...`) 관리.
//!
//! - `GET    /v1/projects/{project_id}/api-keys` (멤버): prefix·시각만. 평문·해시는 내보내지 않는다.
//! - `POST   /v1/projects/{project_id}/api-keys` (admin): 201, 평문 키는 이 응답에서 한 번만.
//! - `DELETE /v1/projects/{project_id}/api-keys/{key_id}` (admin): 204 멱등. 다른 프로젝트의
//!   키 id는 이 프로젝트 조건에 걸리지 않으므로 아무 일도 일어나지 않는다 (존재 여부도 알 수 없다).

use super::{audit, ts};
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ProjectAccess, parse_id};
use analytics_core::dto::admin::{ApiKeyDto, CreatedApiKeyResponse, Role};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::json;
use std::collections::HashMap;

/// 프로젝트당 폐기되지 않은 키 상한.
pub const MAX_ACTIVE_KEYS: i64 = 20;

fn dto(k: analytics_pg::api_keys::ApiKey) -> ApiKeyDto {
    ApiKeyDto {
        id: k.id,
        prefix: k.prefix,
        scopes: k.scopes,
        created_at: ts(k.created_at),
        last_used_at: k.last_used_at.map(ts),
        revoked_at: k.revoked_at.map(ts),
    }
}

pub async fn list(
    State(state): State<AppState>,
    access: ProjectAccess,
) -> ApiResult<Json<Vec<ApiKeyDto>>> {
    let keys = analytics_pg::api_keys::list(&state.pg, access.id()).await?;
    Ok(Json(keys.into_iter().map(dto).collect()))
}

pub async fn create(
    State(state): State<AppState>,
    access: ProjectAccess,
) -> ApiResult<(StatusCode, Json<CreatedApiKeyResponse>)> {
    access.require(Role::Admin)?;
    // 동시 요청 두 개가 함께 통과하면 상한을 하나 넘을 수 있다 (관리자 수동 작업이라 허용).
    if analytics_pg::api_keys::count_active(&state.pg, access.id()).await? >= MAX_ACTIVE_KEYS {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "limit_exceeded",
            format!(
                "사용 중인 비밀 키는 프로젝트당 최대 {MAX_ACTIVE_KEYS}개입니다. 안 쓰는 키를 폐기하세요"
            ),
        ));
    }
    let mut tx = state.pg.begin().await?;
    let (secret, key) =
        analytics_pg::api_keys::create(&mut *tx, access.id(), Some(access.user.id())).await?;
    // 감사 로그에는 prefix만 남긴다 (평문 키 금지).
    audit(
        &mut *tx,
        Some(access.project.org_id),
        access.user.id(),
        "api_key.create",
        format!("api_key:{}", key.id),
        json!({ "project_id": access.id(), "prefix": key.prefix }),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedApiKeyResponse {
            key: dto(key),
            secret,
        }),
    ))
}

pub async fn revoke(
    State(state): State<AppState>,
    access: ProjectAccess,
    Path(params): Path<HashMap<String, String>>,
) -> ApiResult<StatusCode> {
    access.require(Role::Admin)?;
    let id: i64 = parse_id(params.get("key_id").map(String::as_str).unwrap_or(""))?;
    let mut tx = state.pg.begin().await?;
    if analytics_pg::api_keys::revoke(&mut *tx, access.id(), id).await? {
        audit(
            &mut *tx,
            Some(access.project.org_id),
            access.user.id(),
            "api_key.revoke",
            format!("api_key:{id}"),
            json!({ "project_id": access.id() }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

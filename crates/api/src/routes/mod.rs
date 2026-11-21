//! 라우트 핸들러와 DB 행 → DTO 변환.

pub mod ad_spend;
pub mod api_keys;
pub mod auth;
pub mod orgs;
pub mod project_config;
pub mod projects;
pub mod reports;
pub mod users;

use crate::AppState;
use crate::error::ApiResult;
use analytics_core::dto::admin::{OrgDto, ProjectDto, ProjectStatus, Role, UserDto};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// RFC 3339 (UTC).
pub fn ts(t: OffsetDateTime) -> String {
    t.to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .unwrap_or_default()
}

pub fn user_dto(u: &analytics_pg::users::User) -> UserDto {
    UserDto {
        id: u.id,
        email: u.email.clone(),
        name: u.name.clone(),
        is_super_admin: u.is_super_admin,
        created_at: ts(u.created_at),
        last_login_at: u.last_login_at.map(ts),
    }
}

pub fn org_dto(o: &analytics_pg::orgs::Org, role: Role) -> OrgDto {
    OrgDto {
        id: o.id,
        name: o.name.clone(),
        memo: o.memo.clone(),
        created_at: ts(o.created_at),
        role,
    }
}

/// DB 상태 문자열 → DTO. `deleted`는 조회 단계에서 걸러지므로 여기 오지 않는다.
pub fn status(s: &str) -> ProjectStatus {
    ProjectStatus::parse(s).unwrap_or(ProjectStatus::Paused)
}

pub fn project_dto(p: &analytics_pg::projects::Project, role: Role) -> ProjectDto {
    ProjectDto {
        id: p.id,
        org_id: p.org_id,
        name: p.name.clone(),
        public_key: p.public_key.clone(),
        timezone: p.timezone.clone(),
        currency: p.currency.clone(),
        status: status(&p.status),
        settings_version: p.settings_version,
        created_at: ts(p.created_at),
        role,
    }
}

/// 감사 로그 기록.
pub async fn audit(
    state: &AppState,
    org_id: Option<i64>,
    user_id: i64,
    action: &str,
    target: String,
    payload: serde_json::Value,
) -> ApiResult<()> {
    analytics_pg::audit::record(
        &state.pg,
        analytics_pg::audit::Entry {
            org_id,
            user_id: Some(user_id),
            action,
            target,
            payload,
        },
    )
    .await?;
    Ok(())
}

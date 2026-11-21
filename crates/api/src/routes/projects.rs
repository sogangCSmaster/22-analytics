//! 프로젝트와 스니펫.
//!
//! - `GET /v1/orgs/{org_id}/projects`: 멤버
//! - `POST /v1/orgs/{org_id}/projects`: 광고주 admin
//! - `GET /v1/projects/{project_id}`: 멤버
//! - `PATCH /v1/projects/{project_id}`: 광고주 admin (name, status, timezone)
//! - `GET /v1/projects/{project_id}/snippet`: 멤버

use super::{audit, project_dto};
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, OrgAccess, ProjectAccess};
use crate::validate;
use analytics_core::dto::admin::{
    CreateProjectRequest, ProjectDto, Role, SnippetResponse, UpdateProjectRequest,
};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;

pub const DEFAULT_TIMEZONE: &str = "Asia/Seoul";
pub const DEFAULT_CURRENCY: &str = "KRW";

async fn timezone(state: &AppState, tz: &str) -> ApiResult<String> {
    let tz = validate::timezone_shape(tz)?;
    if analytics_pg::projects::is_valid_timezone(&state.pg, &tz).await? {
        Ok(tz)
    } else {
        Err(ApiError::invalid("timezone: IANA 타임존 이름이 아닙니다"))
    }
}

pub async fn list(
    State(state): State<AppState>,
    access: OrgAccess,
) -> ApiResult<Json<Vec<ProjectDto>>> {
    let rows = analytics_pg::projects::list_by_org(&state.pg, access.id()).await?;
    Ok(Json(
        rows.iter().map(|p| project_dto(p, access.role)).collect(),
    ))
}

pub async fn create(
    State(state): State<AppState>,
    access: OrgAccess,
    ApiJson(req): ApiJson<CreateProjectRequest>,
) -> ApiResult<(StatusCode, Json<ProjectDto>)> {
    access.require(Role::Admin)?;
    let name = validate::name("name", &req.name, validate::MAX_NAME_CHARS)?;
    let tz = timezone(&state, req.timezone.as_deref().unwrap_or(DEFAULT_TIMEZONE)).await?;
    let currency = validate::currency(req.currency.as_deref().unwrap_or(DEFAULT_CURRENCY))?;
    let mut tx = state.pg.begin().await?;
    let p = analytics_pg::projects::create(&mut *tx, access.id(), &name, &tz, &currency).await?;
    audit(
        &mut *tx,
        Some(access.id()),
        access.user.id(),
        "project.create",
        format!("project:{}", p.id),
        json!({ "name": p.name, "timezone": p.timezone, "currency": p.currency }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(project_dto(&p, access.role))))
}

pub async fn get(access: ProjectAccess) -> Json<ProjectDto> {
    Json(project_dto(&access.project, access.role))
}

pub async fn update(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiJson(req): ApiJson<UpdateProjectRequest>,
) -> ApiResult<Json<ProjectDto>> {
    access.require(Role::Admin)?;
    let name = req
        .name
        .as_deref()
        .map(|n| validate::name("name", n, validate::MAX_NAME_CHARS))
        .transpose()?;
    let tz = match req.timezone.as_deref() {
        Some(t) => Some(timezone(&state, t).await?),
        None => None,
    };
    let status = req.status.map(|s| s.as_str());
    let mut tx = state.pg.begin().await?;
    let p = analytics_pg::projects::update(
        &mut *tx,
        access.id(),
        name.as_deref(),
        status,
        tz.as_deref(),
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    audit(
        &mut *tx,
        Some(p.org_id),
        access.user.id(),
        "project.update",
        format!("project:{}", p.id),
        json!({ "name": name, "status": status, "timezone": tz }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(project_dto(&p, access.role)))
}

/// 설치 스니펫. 큐 스텁을 먼저 둬서 로드 전 `at(...)` 호출도 잃지 않는다.
pub fn snippet_html(tracker_host: &str, public_key: &str) -> String {
    format!(
        "<script>window.at=window.at||function(){{(at.q=at.q||[]).push(arguments)}};</script>\n\
         <script async src=\"{tracker_host}/t.js\" data-key=\"{public_key}\"></script>"
    )
}

pub async fn snippet(
    State(state): State<AppState>,
    access: ProjectAccess,
) -> Json<SnippetResponse> {
    let host = state.config.tracker_host.trim_end_matches('/').to_owned();
    let pk = access.project.public_key.clone();
    Json(SnippetResponse {
        html: snippet_html(&host, &pk),
        public_key: pk,
        tracker_host: host,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_format() {
        let h = snippet_html("https://c.test", "pk_abc12345");
        assert!(h.starts_with(
            "<script>window.at=window.at||function(){(at.q=at.q||[]).push(arguments)};</script>"
        ));
        assert!(h.ends_with(
            r#"<script async src="https://c.test/t.js" data-key="pk_abc12345"></script>"#
        ));
    }
}

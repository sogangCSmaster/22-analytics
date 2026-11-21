//! 프로젝트 수집 설정·허용 도메인·채널 규칙·기여 설정.
//! 읽기는 멤버 누구나, 변경은 `analyst` 이상.

use super::audit;
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ProjectAccess, parse_id};
use crate::validate;
use analytics_core::attribution::Model;
use analytics_core::dto::admin::{
    AddOriginRequest, AttributionSettingsDto, ChannelRuleDto, CreateChannelRuleRequest,
    OriginsResponse, ProjectSettingsResponse, Role, UpdateChannelRuleRequest,
};
use analytics_core::settings::TrackingSettings;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::json;
use std::collections::HashMap;

fn target(access: &ProjectAccess) -> String {
    format!("project:{}", access.id())
}

// ---- 수집 설정 ------------------------------------------------------------

pub async fn get_settings(
    State(state): State<AppState>,
    access: ProjectAccess,
) -> ApiResult<Json<ProjectSettingsResponse>> {
    let (settings, settings_version) = analytics_pg::projects::settings(&state.pg, access.id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(ProjectSettingsResponse {
        settings,
        settings_version,
    }))
}

/// 전체 교체. 생략한 필드는 기본값, 모르는 필드는 무시(저장 안 함).
/// 값이 같으면 버전과 감사 로그를 건드리지 않는다 (멱등).
pub async fn put_settings(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiJson(req): ApiJson<TrackingSettings>,
) -> ApiResult<Json<ProjectSettingsResponse>> {
    access.require(Role::Analyst)?;
    let new = validate::tracking_settings(req)?;
    let mut tx = state.pg.begin().await?;
    let (settings, settings_version, changed) =
        analytics_pg::projects::put_settings(&mut *tx, access.id(), &new)
            .await?
            .ok_or_else(ApiError::not_found)?;
    if changed {
        audit(
            &mut *tx,
            Some(access.project.org_id),
            access.user.id(),
            "project.settings.update",
            target(&access),
            json!({ "settings": settings, "settings_version": settings_version }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(ProjectSettingsResponse {
        settings,
        settings_version,
    }))
}

// ---- 허용 도메인 ----------------------------------------------------------

pub async fn list_origins(
    State(state): State<AppState>,
    access: ProjectAccess,
) -> ApiResult<Json<OriginsResponse>> {
    let origins = analytics_pg::projects::origins(&state.pg, access.id()).await?;
    Ok(Json(OriginsResponse { origins }))
}

/// 새로 추가하면 201, 이미 있으면 200 (멱등). 응답은 전체 목록.
pub async fn add_origin(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiJson(req): ApiJson<AddOriginRequest>,
) -> ApiResult<(StatusCode, Json<OriginsResponse>)> {
    access.require(Role::Analyst)?;
    let origin = validate::origin(&req.origin)?;
    let mut tx = state.pg.begin().await?;
    let created = analytics_pg::projects::add_origin(&mut *tx, access.id(), &origin).await?;
    if created {
        audit(
            &mut *tx,
            Some(access.project.org_id),
            access.user.id(),
            "origin.add",
            target(&access),
            json!({ "origin": origin }),
        )
        .await?;
    }
    tx.commit().await?;
    let origins = analytics_pg::projects::origins(&state.pg, access.id()).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(OriginsResponse { origins })))
}

/// 제거. 없어도 204 (멱등). 경로의 도메인도 같은 규칙으로 정규화한다.
pub async fn remove_origin(
    State(state): State<AppState>,
    access: ProjectAccess,
    Path(params): Path<HashMap<String, String>>,
) -> ApiResult<StatusCode> {
    access.require(Role::Analyst)?;
    let raw = params.get("origin").map(String::as_str).unwrap_or("");
    let Ok(origin) = validate::origin(raw) else {
        return Ok(StatusCode::NO_CONTENT);
    };
    let mut tx = state.pg.begin().await?;
    if analytics_pg::projects::remove_origin(&mut *tx, access.id(), &origin).await? {
        audit(
            &mut *tx,
            Some(access.project.org_id),
            access.user.id(),
            "origin.remove",
            target(&access),
            json!({ "origin": origin }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- 채널 규칙 ------------------------------------------------------------

fn rule_dto(r: analytics_pg::channel_rules::Rule) -> ChannelRuleDto {
    ChannelRuleDto {
        id: r.id,
        priority: r.priority,
        channel: r.channel,
        when: r.when,
        label: r.label,
    }
}

/// 채널 이름: 광고비 CSV와 같이 소문자로 저장해야 매출 리포트에서 광고비와 한 채널로 묶인다.
fn channel_name(s: &str) -> ApiResult<String> {
    Ok(validate::name("channel", s, validate::MAX_CHANNEL_CHARS)?.to_lowercase())
}

fn rule_id(params: &HashMap<String, String>) -> ApiResult<i64> {
    parse_id(params.get("rule_id").map(String::as_str).unwrap_or(""))
}

pub async fn list_rules(
    State(state): State<AppState>,
    access: ProjectAccess,
) -> ApiResult<Json<Vec<ChannelRuleDto>>> {
    let rules = analytics_pg::channel_rules::list(&state.pg, access.id()).await?;
    Ok(Json(rules.into_iter().map(rule_dto).collect()))
}

pub async fn create_rule(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiJson(req): ApiJson<CreateChannelRuleRequest>,
) -> ApiResult<(StatusCode, Json<ChannelRuleDto>)> {
    access.require(Role::Analyst)?;
    let channel = channel_name(&req.channel)?;
    let when = validate::rule_match(req.when)?;
    let priority = validate::priority(req.priority.unwrap_or(100))?;
    let label = validate::optional_name("label", &req.label, validate::MAX_NAME_CHARS)?;
    let mut tx = state.pg.begin().await?;
    let rule = analytics_pg::channel_rules::create(
        &mut *tx,
        access.id(),
        priority,
        &channel,
        &when,
        &label,
    )
    .await?;
    audit(
        &mut *tx,
        Some(access.project.org_id),
        access.user.id(),
        "channel_rule.create",
        format!("channel_rule:{}", rule.id),
        json!({ "project_id": access.id(), "channel": rule.channel, "match": rule.when, "priority": rule.priority }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(rule_dto(rule))))
}

pub async fn update_rule(
    State(state): State<AppState>,
    access: ProjectAccess,
    Path(params): Path<HashMap<String, String>>,
    ApiJson(req): ApiJson<UpdateChannelRuleRequest>,
) -> ApiResult<Json<ChannelRuleDto>> {
    access.require(Role::Analyst)?;
    let id = rule_id(&params)?;
    let mut rule = analytics_pg::channel_rules::get(&state.pg, access.id(), id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if let Some(c) = req.channel.as_deref() {
        rule.channel = channel_name(c)?;
    }
    if let Some(m) = req.when {
        rule.when = validate::rule_match(m)?;
    }
    if let Some(p) = req.priority {
        rule.priority = validate::priority(p)?;
    }
    if let Some(l) = req.label.as_deref() {
        rule.label = validate::optional_name("label", l, validate::MAX_NAME_CHARS)?;
    }
    let mut tx = state.pg.begin().await?;
    let rule = analytics_pg::channel_rules::update(&mut *tx, access.id(), &rule)
        .await?
        .ok_or_else(ApiError::not_found)?;
    audit(
        &mut *tx,
        Some(access.project.org_id),
        access.user.id(),
        "channel_rule.update",
        format!("channel_rule:{}", rule.id),
        json!({ "project_id": access.id(), "channel": rule.channel, "match": rule.when, "priority": rule.priority }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(rule_dto(rule)))
}

/// 삭제. 없어도 204 (멱등). 숫자가 아닌 id는 404.
/// 다른 프로젝트의 규칙 id는 `project_id` 조건 때문에 지워지지 않고, 없는 id와 똑같이 204를
/// 돌려준다 (존재 여부를 알려주지 않는다).
pub async fn delete_rule(
    State(state): State<AppState>,
    access: ProjectAccess,
    Path(params): Path<HashMap<String, String>>,
) -> ApiResult<StatusCode> {
    access.require(Role::Analyst)?;
    let id = rule_id(&params)?;
    let mut tx = state.pg.begin().await?;
    if analytics_pg::channel_rules::delete(&mut *tx, access.id(), id).await? {
        audit(
            &mut *tx,
            Some(access.project.org_id),
            access.user.id(),
            "channel_rule.delete",
            format!("channel_rule:{id}"),
            json!({ "project_id": access.id() }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- 기여 설정 ------------------------------------------------------------

fn attribution_dto(s: analytics_pg::attribution::AttributionSettings) -> AttributionSettingsDto {
    let model = serde_json::from_value(json!(s.default_model)).unwrap_or(Model::LastClick);
    AttributionSettingsDto {
        lookback_days: s.lookback_days,
        default_model: model,
        half_life_hours: s.half_life_hours,
        include_organic: s.include_organic,
        include_referral: s.include_referral,
    }
}

pub async fn get_attribution(
    State(state): State<AppState>,
    access: ProjectAccess,
) -> ApiResult<Json<AttributionSettingsDto>> {
    let s = analytics_pg::attribution::get_or_default(&state.pg, access.id()).await?;
    Ok(Json(attribution_dto(s)))
}

pub async fn put_attribution(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiJson(req): ApiJson<AttributionSettingsDto>,
) -> ApiResult<Json<AttributionSettingsDto>> {
    access.require(Role::Analyst)?;
    if !req.default_model.conserves_revenue() {
        return Err(ApiError::invalid(
            "default_model: 매출 보존 모델만 기본값으로 쓸 수 있습니다",
        ));
    }
    let s = analytics_pg::attribution::AttributionSettings {
        lookback_days: validate::lookbacks(&req.lookback_days)?,
        default_model: req.default_model.as_str().to_owned(),
        half_life_hours: validate::half_life(req.half_life_hours)?,
        include_organic: req.include_organic,
        include_referral: req.include_referral,
    };
    let current = analytics_pg::attribution::get_or_default(&state.pg, access.id()).await?;
    if current == s {
        return Ok(Json(attribution_dto(current)));
    }
    let mut tx = state.pg.begin().await?;
    let saved = analytics_pg::attribution::put(&mut *tx, access.id(), &s).await?;
    audit(
        &mut *tx,
        Some(access.project.org_id),
        access.user.id(),
        "attribution.update",
        target(&access),
        json!({
            "lookback_days": saved.lookback_days, "default_model": saved.default_model,
            "half_life_hours": saved.half_life_hours, "include_organic": saved.include_organic,
            "include_referral": saved.include_referral,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(attribution_dto(saved)))
}

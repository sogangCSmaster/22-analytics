//! 인증·광고주·프로젝트·키·설정 관리 DTO.
//!
//! API(`analytics-api`)와 대시보드(Dioxus, wasm)가 함께 쓴다. I/O 의존성을 넣지 않는다.
//! 시각은 RFC 3339 문자열(UTC), 날짜는 `YYYY-MM-DD`, 금액은 통화 최소단위 정수다.

use crate::attribution::Model;
use crate::channel::RuleMatch;
use crate::settings::TrackingSettings;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 공통
// ---------------------------------------------------------------------------

/// 모든 오류 응답의 본문. `error`는 기계가 읽는 코드, `message`는 사람이 읽는 설명이다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: String,
    pub message: String,
}

/// 목록 응답 (offset 페이지네이션).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// 광고주(Organization) 안에서의 역할. 순서: `Viewer < Analyst < Admin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// 리포트·설정 읽기만 가능 (광고주 담당자)
    Viewer,
    /// 수집 설정·허용 도메인·채널 규칙·기여 설정·광고비 편집 (대행사 AE)
    Analyst,
    /// 광고주 정보·멤버·프로젝트 관리
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Analyst => "analyst",
            Role::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "viewer" => Some(Role::Viewer),
            "analyst" => Some(Role::Analyst),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 인증
// ---------------------------------------------------------------------------

/// `POST /v1/auth/login`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// 사용자. 비밀번호 해시는 어떤 응답에도 넣지 않는다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserDto {
    pub id: i64,
    pub email: String,
    pub name: String,
    pub is_super_admin: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

/// `GET /v1/auth/me`, `POST /v1/auth/login` 응답.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeResponse {
    pub user: UserDto,
    /// 접근 가능한 광고주. 슈퍼 관리자는 모든 광고주를 `admin`으로 본다.
    pub orgs: Vec<OrgAccess>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrgAccess {
    pub id: i64,
    pub name: String,
    pub role: Role,
    pub projects: Vec<ProjectSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub id: i32,
    pub name: String,
    pub status: ProjectStatus,
}

// ---------------------------------------------------------------------------
// 사용자 (슈퍼 관리자 전용)
// ---------------------------------------------------------------------------

/// `POST /v1/users`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    /// 초기 비밀번호 (10~256자)
    pub password: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_super_admin: bool,
}

// ---------------------------------------------------------------------------
// 광고주
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgDto {
    pub id: i64,
    pub name: String,
    pub memo: String,
    pub created_at: String,
    /// 요청한 사용자의 역할
    pub role: Role,
}

/// `POST /v1/orgs`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateOrgRequest {
    pub name: String,
    #[serde(default)]
    pub memo: String,
}

/// `PATCH /v1/orgs/{id}`. 빠진 필드는 바꾸지 않는다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateOrgRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberDto {
    pub user_id: i64,
    pub email: String,
    pub name: String,
    pub role: Role,
}

/// `POST /v1/orgs/{id}/members`. 이미 멤버면 역할만 바꾼다 (upsert).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddMemberRequest {
    pub email: String,
    pub role: Role,
}

// ---------------------------------------------------------------------------
// 프로젝트
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    Active,
    Paused,
}

impl ProjectStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectStatus::Active => "active",
            ProjectStatus::Paused => "paused",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(ProjectStatus::Active),
            "paused" => Some(ProjectStatus::Paused),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDto {
    pub id: i32,
    pub org_id: i64,
    pub name: String,
    pub public_key: String,
    pub timezone: String,
    pub currency: String,
    pub status: ProjectStatus,
    pub settings_version: i32,
    pub created_at: String,
    /// 요청한 사용자의 역할
    pub role: Role,
}

/// `POST /v1/orgs/{id}/projects`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    /// IANA 타임존 (기본 `Asia/Seoul`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// ISO 4217 (기본 `KRW`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

/// `PATCH /v1/projects/{id}`. 빠진 필드는 바꾸지 않는다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateProjectRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ProjectStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

/// `GET/PUT /v1/projects/{id}/settings` 응답. PUT 본문은 [`TrackingSettings`] 그대로다.
/// 값이 실제로 바뀐 경우에만 `settings_version`이 1 오른다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettingsResponse {
    pub settings: TrackingSettings,
    pub settings_version: i32,
}

// ---------------------------------------------------------------------------
// 허용 도메인
// ---------------------------------------------------------------------------

/// `POST /v1/projects/{id}/origins`. 스킴·경로·포트 없는 도메인 (`client.com`은 하위 도메인 포함).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddOriginRequest {
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginsResponse {
    pub origins: Vec<String>,
}

// ---------------------------------------------------------------------------
// 채널 규칙
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelRuleDto {
    pub id: i64,
    pub priority: i32,
    pub channel: String,
    #[serde(rename = "match")]
    pub when: RuleMatch,
    pub label: String,
}

/// `POST /v1/projects/{id}/channel-rules`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateChannelRuleRequest {
    pub channel: String,
    #[serde(rename = "match")]
    pub when: RuleMatch,
    /// 낮을수록 먼저 평가 (기본 100)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default)]
    pub label: String,
}

/// `PATCH /v1/projects/{id}/channel-rules/{rule_id}`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateChannelRuleRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(rename = "match", default, skip_serializing_if = "Option::is_none")]
    pub when: Option<RuleMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

// ---------------------------------------------------------------------------
// 기여 설정
// ---------------------------------------------------------------------------

/// `GET/PUT /v1/projects/{id}/attribution-settings`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributionSettingsDto {
    /// 1..=90 사이 일 수. 저장 시 정렬·중복 제거한다.
    pub lookback_days: Vec<i16>,
    /// 매출 보존 모델만 허용 (`raw_sum` 불가)
    pub default_model: Model,
    /// 1..=2160 (90일)
    pub half_life_hours: i32,
    pub include_organic: bool,
    pub include_referral: bool,
}

// ---------------------------------------------------------------------------
// 스니펫
// ---------------------------------------------------------------------------

/// `GET /v1/projects/{id}/snippet`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetResponse {
    pub public_key: String,
    pub tracker_host: String,
    pub html: String,
}

// ---------------------------------------------------------------------------
// 광고비
// ---------------------------------------------------------------------------

/// `POST /v1/projects/{id}/ad-spend` 응답 (본문은 CSV `day,channel,campaign,amount`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdSpendUploadResponse {
    /// 반영된 (day, channel, campaign) 행 수. 같은 키가 CSV에 여러 번 나오면 마지막 값 하나로 센다.
    pub rows: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdSpendRow {
    pub day: String,
    pub channel: String,
    pub campaign: String,
    pub amount: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdSpendResponse {
    pub from: String,
    pub to: String,
    pub rows: Vec<AdSpendRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_order_and_serde() {
        assert!(Role::Viewer < Role::Analyst && Role::Analyst < Role::Admin);
        assert_eq!(
            serde_json::to_string(&Role::Analyst).unwrap(),
            "\"analyst\""
        );
        for r in [Role::Viewer, Role::Analyst, Role::Admin] {
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        assert_eq!(Role::parse("owner"), None);
    }

    #[test]
    fn channel_rule_uses_match_key() {
        let r: CreateChannelRuleRequest =
            serde_json::from_str(r#"{"channel":"naver_sa","match":{"utm_source":["naver"]}}"#)
                .unwrap();
        assert_eq!(r.when.utm_source, vec!["naver".to_string()]);
        assert_eq!(r.priority, None);
    }

    #[test]
    fn user_dto_has_no_password_field() {
        let u = UserDto {
            id: 1,
            email: "a@b.c".into(),
            name: String::new(),
            is_super_admin: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            last_login_at: None,
        };
        let s = serde_json::to_string(&u).unwrap();
        assert!(!s.contains("password"));
    }
}

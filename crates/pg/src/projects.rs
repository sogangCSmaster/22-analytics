use analytics_core::channel::{ChannelRule, RuleMatch};
use analytics_core::collect::ProjectConfig;
use analytics_core::settings::TrackingSettings;
use sqlx::{PgPool, Row};

/// 공개 키로 수집 설정을 읽는다. Worker KV에 올라가는 것과 같은 구조다.
pub async fn config_by_public_key(
    pool: &PgPool,
    public_key: &str,
) -> anyhow::Result<Option<ProjectConfig>> {
    let Some(row) = sqlx::query(
        "SELECT id, public_key, status, salt, settings, settings_version FROM projects WHERE public_key = $1",
    )
    .bind(public_key)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    let project_id: i32 = row.get("id");
    let settings: serde_json::Value = row.get("settings");
    let settings: TrackingSettings = serde_json::from_value(settings).unwrap_or_default();

    let origins: Vec<String> = sqlx::query_scalar(
        "SELECT origin FROM project_origins WHERE project_id = $1 ORDER BY origin",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    // 프로젝트 규칙 → 전역 규칙 순서. 각 그룹 안에서는 priority 오름차순.
    let rules = sqlx::query(
        "SELECT channel, match FROM channel_rules \
         WHERE project_id = $1 OR project_id IS NULL \
         ORDER BY (project_id IS NULL), priority, id",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .filter_map(|r| {
        let when: RuleMatch = serde_json::from_value(r.get("match")).ok()?;
        Some(ChannelRule {
            channel: r.get("channel"),
            when,
        })
    })
    .collect();

    Ok(Some(ProjectConfig {
        project_id: project_id as u32,
        public_key: row.get("public_key"),
        active: row.get::<String, _>("status") == "active",
        allowed_origins: origins,
        settings,
        settings_version: row.get::<i32, _>("settings_version") as u32,
        channel_rules: rules,
        salt_hex: row.get("salt"),
    }))
}

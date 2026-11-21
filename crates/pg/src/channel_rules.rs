//! 프로젝트 채널 규칙 (`channel_rules`, project_id NOT NULL인 행만).
//! 전역 기본 규칙(project_id NULL)은 관리 API로 바꾸지 않는다.

use analytics_core::channel::RuleMatch;
use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: i64,
    pub priority: i32,
    pub channel: String,
    pub when: RuleMatch,
    pub label: String,
}

type RuleRow = (i64, i32, String, serde_json::Value, String);

fn from_row((id, priority, channel, m, label): RuleRow) -> Rule {
    Rule {
        id,
        priority,
        channel,
        when: serde_json::from_value(m).unwrap_or_default(),
        label,
    }
}

/// priority 오름차순, 같으면 id 순 (수집 설정과 같은 평가 순서).
pub async fn list(pool: &PgPool, project_id: i32) -> anyhow::Result<Vec<Rule>> {
    let rows = sqlx::query_as::<_, RuleRow>(
        "SELECT id, priority, channel, match, label FROM channel_rules \
         WHERE project_id = $1 ORDER BY priority, id",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(from_row).collect())
}

pub async fn get(pool: &PgPool, project_id: i32, id: i64) -> anyhow::Result<Option<Rule>> {
    let row = sqlx::query_as::<_, RuleRow>(
        "SELECT id, priority, channel, match, label FROM channel_rules \
         WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(from_row))
}

pub async fn create(
    pool: &PgPool,
    project_id: i32,
    priority: i32,
    channel: &str,
    when: &RuleMatch,
    label: &str,
) -> anyhow::Result<Rule> {
    let row = sqlx::query_as::<_, RuleRow>(
        "INSERT INTO channel_rules (project_id, priority, channel, match, label) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id, priority, channel, match, label",
    )
    .bind(project_id)
    .bind(priority)
    .bind(channel)
    .bind(serde_json::to_value(when)?)
    .bind(label)
    .fetch_one(pool)
    .await?;
    Ok(from_row(row))
}

/// 규칙 전체를 덮어쓴다 (호출자가 기존 값과 병합한 결과를 넘긴다).
pub async fn update(pool: &PgPool, project_id: i32, rule: &Rule) -> anyhow::Result<Option<Rule>> {
    let row = sqlx::query_as::<_, RuleRow>(
        "UPDATE channel_rules SET priority = $3, channel = $4, match = $5, label = $6 \
         WHERE project_id = $1 AND id = $2 RETURNING id, priority, channel, match, label",
    )
    .bind(project_id)
    .bind(rule.id)
    .bind(rule.priority)
    .bind(&rule.channel)
    .bind(serde_json::to_value(&rule.when)?)
    .bind(&rule.label)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(from_row))
}

/// 실제로 지웠으면 `true` (멱등).
pub async fn delete(pool: &PgPool, project_id: i32, id: i64) -> anyhow::Result<bool> {
    let n = sqlx::query("DELETE FROM channel_rules WHERE project_id = $1 AND id = $2")
        .bind(project_id)
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

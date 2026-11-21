//! 감사 로그 (`audit_log`). 관리 API의 모든 변경을 남긴다.
//! payload에는 비밀번호·해시·키 같은 비밀 값을 절대 넣지 않는다.

use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct Entry<'a> {
    pub org_id: Option<i64>,
    pub user_id: Option<i64>,
    /// 예: `org.create`, `project.settings.update`
    pub action: &'a str,
    /// 예: `project:12`
    pub target: String,
    pub payload: serde_json::Value,
}

pub async fn record(pool: &PgPool, e: Entry<'_>) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO audit_log (org_id, user_id, action, target, payload) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(e.org_id)
    .bind(e.user_id)
    .bind(e.action)
    .bind(&e.target)
    .bind(&e.payload)
    .execute(pool)
    .await?;
    Ok(())
}

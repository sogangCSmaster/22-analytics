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

// ---------------------------------------------------------------------------
// 관리 API용 프로젝트 CRUD (WP3)
// ---------------------------------------------------------------------------

/// 관리 화면용 프로젝트 행. `salt`는 담지 않는다.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Project {
    pub id: i32,
    pub org_id: i64,
    pub name: String,
    pub public_key: String,
    pub timezone: String,
    pub currency: String,
    pub status: String,
    pub settings_version: i32,
    pub created_at: time::OffsetDateTime,
}

/// 사용자 관점의 프로젝트: 프로젝트 + 광고주 안 역할.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectWithRole {
    pub project: Project,
    pub role: String,
}

const PROJECT_COLS: &str = "p.id, p.org_id, p.name, p.public_key, p.timezone, p.currency, \
                            p.status, p.settings_version, p.created_at";

/// 프로젝트를 만든다: 공개 키·salt 생성, 기본 수집 설정, `attribution_settings` 기본 행.
pub async fn create(
    pool: &PgPool,
    org_id: i64,
    name: &str,
    timezone: &str,
    currency: &str,
) -> anyhow::Result<Project> {
    let mut tx = pool.begin().await?;
    let settings = serde_json::to_value(TrackingSettings::default())?;
    let sql = format!(
        "INSERT INTO projects AS p (org_id, name, public_key, salt, timezone, currency, settings) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {PROJECT_COLS}"
    );
    let project = sqlx::query_as::<_, Project>(sqlx::AssertSqlSafe(sql))
        .bind(org_id)
        .bind(name)
        .bind(crate::new_public_key())
        .bind(crate::new_salt_hex())
        .bind(timezone)
        .bind(currency)
        .bind(settings)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO attribution_settings (project_id) VALUES ($1)")
        .bind(project.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(project)
}

/// 삭제 상태가 아닌 프로젝트.
pub async fn get(pool: &PgPool, id: i32) -> anyhow::Result<Option<Project>> {
    let sql =
        format!("SELECT {PROJECT_COLS} FROM projects p WHERE p.id = $1 AND p.status <> 'deleted'");
    Ok(sqlx::query_as::<_, Project>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// 사용자가 멤버인 광고주의 프로젝트와 역할. 접근할 수 없으면 `None`.
pub async fn get_for_user(
    pool: &PgPool,
    user_id: i64,
    id: i32,
) -> anyhow::Result<Option<ProjectWithRole>> {
    let sql = format!(
        "SELECT {PROJECT_COLS}, m.role FROM projects p \
         JOIN memberships m ON m.org_id = p.org_id AND m.user_id = $1 \
         WHERE p.id = $2 AND p.status <> 'deleted'"
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let Some(r) = row else { return Ok(None) };
    Ok(Some(ProjectWithRole {
        project: <Project as sqlx::FromRow<_>>::from_row(&r)?,
        role: r.try_get("role")?,
    }))
}

/// 광고주의 프로젝트 (삭제 제외, id 순).
pub async fn list_by_org(pool: &PgPool, org_id: i64) -> anyhow::Result<Vec<Project>> {
    list_by_orgs(pool, &[org_id]).await
}

/// 여러 광고주의 프로젝트를 한 번에 (`/v1/auth/me`용).
pub async fn list_by_orgs(pool: &PgPool, org_ids: &[i64]) -> anyhow::Result<Vec<Project>> {
    let sql = format!(
        "SELECT {PROJECT_COLS} FROM projects p \
         WHERE p.org_id = ANY($1) AND p.status <> 'deleted' ORDER BY p.id"
    );
    Ok(sqlx::query_as::<_, Project>(sqlx::AssertSqlSafe(sql))
        .bind(org_ids)
        .fetch_all(pool)
        .await?)
}

/// `None` 필드는 그대로 둔다. 상태 값 검증은 호출자 몫이다 (DB CHECK 제약이 최후 방어선).
pub async fn update(
    pool: &PgPool,
    id: i32,
    name: Option<&str>,
    status: Option<&str>,
    timezone: Option<&str>,
) -> anyhow::Result<Option<Project>> {
    let sql = format!(
        "UPDATE projects AS p SET name = COALESCE($2, p.name), status = COALESCE($3, p.status), \
         timezone = COALESCE($4, p.timezone) \
         WHERE p.id = $1 AND p.status <> 'deleted' RETURNING {PROJECT_COLS}"
    );
    Ok(sqlx::query_as::<_, Project>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(name)
        .bind(status)
        .bind(timezone)
        .fetch_optional(pool)
        .await?)
}

/// Postgres가 아는 IANA 타임존인지.
pub async fn is_valid_timezone(pool: &PgPool, tz: &str) -> anyhow::Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1)")
            .bind(tz)
            .fetch_one(pool)
            .await?,
    )
}

/// 수집 설정과 버전. 저장된 JSON에 빠진 필드는 기본값으로 채운다.
pub async fn settings(pool: &PgPool, id: i32) -> anyhow::Result<Option<(TrackingSettings, i32)>> {
    let row = sqlx::query_as::<_, (serde_json::Value, i32)>(
        "SELECT settings, settings_version FROM projects WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(v, ver)| (serde_json::from_value(v).unwrap_or_default(), ver)))
}

/// 수집 설정 교체. 값이 실제로 바뀐 경우에만 `settings_version`을 1 올린다.
/// 돌려주는 값: (저장된 설정, 버전, 바뀌었는지).
pub async fn put_settings(
    pool: &PgPool,
    id: i32,
    new: &TrackingSettings,
) -> anyhow::Result<Option<(TrackingSettings, i32, bool)>> {
    let mut tx = pool.begin().await?;
    let Some((current, version)) = sqlx::query_as::<_, (serde_json::Value, i32)>(
        "SELECT settings, settings_version FROM projects WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(None);
    };
    let current: TrackingSettings = serde_json::from_value(current).unwrap_or_default();
    if &current == new {
        tx.commit().await?;
        return Ok(Some((current, version, false)));
    }
    let version: i32 = sqlx::query_scalar(
        "UPDATE projects SET settings = $2, settings_version = settings_version + 1 \
         WHERE id = $1 RETURNING settings_version",
    )
    .bind(id)
    .bind(serde_json::to_value(new)?)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some((new.clone(), version, true)))
}

pub async fn origins(pool: &PgPool, id: i32) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT origin FROM project_origins WHERE project_id = $1 ORDER BY origin",
    )
    .bind(id)
    .fetch_all(pool)
    .await?)
}

/// 허용 도메인 추가. 새로 추가됐으면 `true`, 이미 있었으면 `false` (멱등).
pub async fn add_origin(pool: &PgPool, id: i32, origin: &str) -> anyhow::Result<bool> {
    let n = sqlx::query(
        "INSERT INTO project_origins (project_id, origin) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(origin)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// 허용 도메인 제거. 실제로 지웠으면 `true` (멱등).
pub async fn remove_origin(pool: &PgPool, id: i32, origin: &str) -> anyhow::Result<bool> {
    let n = sqlx::query("DELETE FROM project_origins WHERE project_id = $1 AND origin = $2")
        .bind(id)
        .bind(origin)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

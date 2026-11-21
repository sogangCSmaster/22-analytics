//! 광고주(Organization)와 멤버십.
//!
//! 역할 문자열은 `memberships.role` CHECK 제약과 같은 `admin | analyst | viewer`다.

use sqlx::{PgConnection, PgExecutor, PgPool};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Org {
    pub id: i64,
    pub name: String,
    pub memo: String,
    pub created_at: OffsetDateTime,
}

/// 사용자 관점의 광고주: 광고주 + 역할.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OrgWithRole {
    pub id: i64,
    pub name: String,
    pub memo: String,
    pub created_at: OffsetDateTime,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Member {
    pub user_id: i64,
    pub email: String,
    pub name: String,
    pub role: String,
}

pub async fn create<'e>(ex: impl PgExecutor<'e>, name: &str, memo: &str) -> anyhow::Result<Org> {
    Ok(sqlx::query_as::<_, Org>(
        "INSERT INTO organizations (name, memo) VALUES ($1, $2) RETURNING id, name, memo, created_at",
    )
    .bind(name)
    .bind(memo)
    .fetch_one(ex)
    .await?)
}

pub async fn get(pool: &PgPool, id: i64) -> anyhow::Result<Option<Org>> {
    Ok(sqlx::query_as::<_, Org>(
        "SELECT id, name, memo, created_at FROM organizations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// `None` 필드는 그대로 둔다. 광고주가 없으면 `None`.
pub async fn update<'e>(
    ex: impl PgExecutor<'e>,
    id: i64,
    name: Option<&str>,
    memo: Option<&str>,
) -> anyhow::Result<Option<Org>> {
    Ok(sqlx::query_as::<_, Org>(
        "UPDATE organizations SET name = COALESCE($2, name), memo = COALESCE($3, memo) \
         WHERE id = $1 RETURNING id, name, memo, created_at",
    )
    .bind(id)
    .bind(name)
    .bind(memo)
    .fetch_optional(ex)
    .await?)
}

/// 모든 광고주 (슈퍼 관리자용). 역할은 `admin`으로 채운다.
pub async fn list_all(pool: &PgPool) -> anyhow::Result<Vec<OrgWithRole>> {
    Ok(sqlx::query_as::<_, OrgWithRole>(
        "SELECT id, name, memo, created_at, 'admin' AS role FROM organizations ORDER BY id",
    )
    .fetch_all(pool)
    .await?)
}

/// 사용자가 멤버인 광고주.
pub async fn list_for_user(pool: &PgPool, user_id: i64) -> anyhow::Result<Vec<OrgWithRole>> {
    Ok(sqlx::query_as::<_, OrgWithRole>(
        "SELECT o.id, o.name, o.memo, o.created_at, m.role \
         FROM organizations o JOIN memberships m ON m.org_id = o.id \
         WHERE m.user_id = $1 ORDER BY o.id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// 멤버의 광고주와 역할. 멤버가 아니거나 광고주가 없으면 `None`.
pub async fn get_for_user(
    pool: &PgPool,
    user_id: i64,
    org_id: i64,
) -> anyhow::Result<Option<OrgWithRole>> {
    Ok(sqlx::query_as::<_, OrgWithRole>(
        "SELECT o.id, o.name, o.memo, o.created_at, m.role \
         FROM organizations o JOIN memberships m ON m.org_id = o.id \
         WHERE m.user_id = $1 AND o.id = $2",
    )
    .bind(user_id)
    .bind(org_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn members(pool: &PgPool, org_id: i64) -> anyhow::Result<Vec<Member>> {
    Ok(sqlx::query_as::<_, Member>(
        "SELECT u.id AS user_id, u.email, u.name, m.role \
         FROM memberships m JOIN users u ON u.id = m.user_id \
         WHERE m.org_id = $1 ORDER BY u.email",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?)
}

/// 멤버 추가 또는 역할 변경 (멱등).
pub async fn upsert_member<'e>(
    ex: impl PgExecutor<'e>,
    org_id: i64,
    user_id: i64,
    role: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO memberships (user_id, org_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, org_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(user_id)
    .bind(org_id)
    .bind(role)
    .execute(ex)
    .await?;
    Ok(())
}

/// 멤버 제거. 실제로 지웠으면 `true` (없으면 `false`, 멱등).
pub async fn remove_member<'e>(
    ex: impl PgExecutor<'e>,
    org_id: i64,
    user_id: i64,
) -> anyhow::Result<bool> {
    let n = sqlx::query("DELETE FROM memberships WHERE org_id = $1 AND user_id = $2")
        .bind(org_id)
        .bind(user_id)
        .execute(ex)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// 트랜잭션 안에서 광고주 행을 잠근다 (멤버십 변경을 광고주 단위로 직렬화).
/// 광고주가 없으면 `false`.
pub async fn lock(conn: &mut PgConnection, org_id: i64) -> anyhow::Result<bool> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT id FROM organizations WHERE id = $1 FOR UPDATE")
            .bind(org_id)
            .fetch_optional(conn)
            .await?
            .is_some(),
    )
}

/// 사용자가 멤버인 광고주 id.
pub async fn membership_org_ids<'e>(
    ex: impl PgExecutor<'e>,
    user_id: i64,
) -> anyhow::Result<Vec<i64>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM memberships WHERE user_id = $1 ORDER BY org_id")
            .bind(user_id)
            .fetch_all(ex)
            .await?,
    )
}

/// 이 광고주에서 사용자의 역할과 슈퍼 관리자 여부. 멤버가 아니면 `None`.
pub async fn member_role<'e>(
    ex: impl PgExecutor<'e>,
    org_id: i64,
    user_id: i64,
) -> anyhow::Result<Option<(String, bool)>> {
    Ok(sqlx::query_as(
        "SELECT m.role, u.is_super_admin FROM memberships m JOIN users u ON u.id = m.user_id \
         WHERE m.org_id = $1 AND m.user_id = $2",
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_optional(ex)
    .await?)
}

/// 슈퍼 관리자가 아닌 admin 멤버 수.
pub async fn count_member_admins<'e>(ex: impl PgExecutor<'e>, org_id: i64) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM memberships m JOIN users u ON u.id = m.user_id \
         WHERE m.org_id = $1 AND m.role = 'admin' AND NOT u.is_super_admin",
    )
    .bind(org_id)
    .fetch_one(ex)
    .await?)
}

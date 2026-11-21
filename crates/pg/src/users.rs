//! 대시보드 사용자와 비밀번호 해시(argon2id).
//!
//! 비밀번호 해시는 이 모듈 밖으로 나가는 구조체([`User`])에 담지 않는다.
//! 로그인 검증에 필요한 해시는 [`credentials_by_email`]로만 읽는다.

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use sqlx::{PgExecutor, PgPool};
use std::sync::LazyLock;
use time::OffsetDateTime;

/// 새 사용자. `password_hash`는 [`hash_password`] 결과(PHC 문자열)다.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    pub password_hash: String,
    pub name: String,
    pub is_super_admin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub name: String,
    pub is_super_admin: bool,
    pub created_at: OffsetDateTime,
    pub last_login_at: Option<OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateUserError {
    #[error("이미 사용 중인 이메일")]
    EmailTaken,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// argon2id(기본 파라미터: m=19MiB, t=2, p=1) + 무작위 salt로 해시한다.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let hash = Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!("비밀번호 해시 실패: {e}"))?;
    Ok(hash.to_string())
}

/// 해시 검증. 해시 형식이 깨졌으면 `false`. 비교는 argon2 내부에서 상수 시간으로 한다.
pub fn verify_password(password: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// 없는 계정으로 로그인할 때도 같은 비용의 검증을 한 번 수행한다 (사용자 열거 방지).
pub fn dummy_verify(password: &str) {
    static DUMMY: LazyLock<String> = LazyLock::new(|| {
        hash_password("timing-equalizer-not-a-real-password").expect("argon2 기본 파라미터")
    });
    let _ = verify_password(password, &DUMMY);
}

/// 이메일은 호출자가 정규화(trim, 소문자)해서 넘긴다.
pub async fn create<'e>(ex: impl PgExecutor<'e>, u: &NewUser) -> Result<User, CreateUserError> {
    let res = sqlx::query_as::<_, User>(
        "INSERT INTO users (email, password_hash, name, is_super_admin) VALUES ($1, $2, $3, $4) \
         RETURNING id, email, name, is_super_admin, created_at, last_login_at",
    )
    .bind(&u.email)
    .bind(&u.password_hash)
    .bind(&u.name)
    .bind(u.is_super_admin)
    .fetch_one(ex)
    .await;
    match res {
        Ok(user) => Ok(user),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            Err(CreateUserError::EmailTaken)
        }
        Err(e) => Err(CreateUserError::Other(e.into())),
    }
}

/// 로그인 검증용 (id, 해시). 대소문자 무시.
pub async fn credentials_by_email(
    pool: &PgPool,
    email: &str,
) -> anyhow::Result<Option<(i64, String)>> {
    Ok(sqlx::query_as::<_, (i64, String)>(
        "SELECT id, password_hash FROM users WHERE lower(email) = lower($1)",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?)
}

pub async fn by_id<'e>(ex: impl PgExecutor<'e>, id: i64) -> anyhow::Result<Option<User>> {
    Ok(sqlx::query_as::<_, User>(
        "SELECT id, email, name, is_super_admin, created_at, last_login_at FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(ex)
    .await?)
}

pub async fn by_email(pool: &PgPool, email: &str) -> anyhow::Result<Option<User>> {
    Ok(sqlx::query_as::<_, User>(
        "SELECT id, email, name, is_super_admin, created_at, last_login_at FROM users \
         WHERE lower(email) = lower($1)",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?)
}

/// 슈퍼 관리자로 승격. `new_hash`가 있으면 비밀번호도 바꾸고 기존 세션을 모두 끊는다.
pub async fn promote_super_admin(
    pool: &PgPool,
    id: i64,
    new_hash: Option<&str>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE users SET is_super_admin = TRUE, password_hash = COALESCE($2, password_hash) \
         WHERE id = $1",
    )
    .bind(id)
    .bind(new_hash)
    .execute(&mut *tx)
    .await?;
    if new_hash.is_some() {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn touch_login<'e>(ex: impl PgExecutor<'e>, id: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
        .bind(id)
        .execute(ex)
        .await?;
    Ok(())
}

/// 이메일·이름 부분 일치 검색 (`q`는 LIKE 와일드카드를 이스케이프해 글자 그대로 찾는다).
pub async fn list(
    pool: &PgPool,
    q: Option<&str>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<(Vec<User>, i64)> {
    let pattern = q.map(|q| format!("%{}%", escape_like(q)));
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users \
         WHERE $1::text IS NULL OR email ILIKE $1 ESCAPE '\\' OR name ILIKE $1 ESCAPE '\\'",
    )
    .bind(&pattern)
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query_as::<_, User>(
        "SELECT id, email, name, is_super_admin, created_at, last_login_at FROM users \
         WHERE $1::text IS NULL OR email ILIKE $1 ESCAPE '\\' OR name ILIKE $1 ESCAPE '\\' \
         ORDER BY id LIMIT $2 OFFSET $3",
    )
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((rows, total))
}

/// LIKE 패턴 이스케이프 (`\`, `%`, `_`).
pub fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_roundtrip_and_format() {
        let h = hash_password("correct horse").unwrap();
        assert!(h.starts_with("$argon2id$"), "{h}");
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("correct horsf", &h));
        assert!(!verify_password("correct horse", "not-a-phc-string"));
        // 같은 비밀번호라도 salt가 달라 해시가 다르다.
        assert_ne!(h, hash_password("correct horse").unwrap());
    }

    #[test]
    fn escapes_like_wildcards() {
        assert_eq!(escape_like(r"a%b_c\d"), r"a\%b\_c\\d");
        assert_eq!(escape_like("김"), "김");
    }
}

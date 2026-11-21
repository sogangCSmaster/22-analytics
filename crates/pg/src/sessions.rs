//! 대시보드 로그인 세션.
//!
//! 쿠키에는 무작위 32바이트 토큰(hex 64자)을 넣고, DB `sessions.id`에는 `sha256(token)`만 저장한다.
//! DB가 유출돼도 저장값으로는 세션을 만들 수 없고, 조회가 해시 인덱스 일치라서
//! 토큰 비교 시간으로 값을 추측할 수도 없다.

use crate::users::User;
use sha2::{Digest, Sha256};
use sqlx::{PgExecutor, PgPool};

/// 세션 유효 기간 (일)
pub const TTL_DAYS: i64 = 14;
/// 남은 기간이 이보다 짧으면 조회할 때 [`TTL_DAYS`]로 연장한다 (sliding).
pub const REFRESH_BELOW_DAYS: i64 = 7;

/// 새 토큰: 32바이트 난수의 소문자 hex.
pub fn new_token() -> String {
    let bytes: [u8; 32] = rand::random();
    hex::encode(bytes)
}

/// 토큰 형식 검사 (소문자 hex 64자). 형식이 틀리면 DB를 조회하지 않는다.
pub fn is_well_formed(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// 세션을 만들고 쿠키에 넣을 토큰을 돌려준다.
pub async fn create<'e>(ex: impl PgExecutor<'e>, user_id: i64) -> anyhow::Result<String> {
    let token = new_token();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, expires_at) \
         VALUES ($1, $2, now() + make_interval(days => $3::int))",
    )
    .bind(token_hash(&token))
    .bind(user_id)
    .bind(TTL_DAYS as i32)
    .execute(ex)
    .await?;
    Ok(token)
}

/// 유효한 세션의 사용자. 만료 시각이 지금이거나 지났으면 무효다.
/// 사용자가 삭제되면 세션도 함께 지워진다 (FK cascade).
pub async fn lookup(pool: &PgPool, token: &str) -> anyhow::Result<Option<User>> {
    Ok(lookup_refreshing(pool, token).await?.map(|(u, _)| u))
}

/// [`lookup`]과 같고, 이번 조회로 만료를 연장했는지(쿠키도 다시 보내야 하는지)를 함께 돌려준다.
pub async fn lookup_refreshing(pool: &PgPool, token: &str) -> anyhow::Result<Option<(User, bool)>> {
    if !is_well_formed(token) {
        return Ok(None);
    }
    let id = token_hash(token);
    let row = sqlx::query_as::<
        _,
        (
            bool,
            i64,
            String,
            String,
            bool,
            time::OffsetDateTime,
            Option<time::OffsetDateTime>,
        ),
    >(
        "SELECT s.expires_at < now() + make_interval(days => $2::int), \
                u.id, u.email, u.name, u.is_super_admin, u.created_at, u.last_login_at \
         FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.id = $1 AND s.expires_at > now()",
    )
    .bind(&id)
    .bind(REFRESH_BELOW_DAYS as i32)
    .fetch_optional(pool)
    .await?;
    let Some((needs_refresh, uid, email, name, is_super_admin, created_at, last_login_at)) = row
    else {
        return Ok(None);
    };
    let mut refreshed = false;
    if needs_refresh {
        refreshed = sqlx::query(
            "UPDATE sessions SET expires_at = now() + make_interval(days => $2::int) \
             WHERE id = $1 AND expires_at > now()",
        )
        .bind(&id)
        .bind(TTL_DAYS as i32)
        .execute(pool)
        .await?
        .rows_affected()
            > 0;
    }
    let user = User {
        id: uid,
        email,
        name,
        is_super_admin,
        created_at,
        last_login_at,
    };
    Ok(Some((user, refreshed)))
}

/// 세션 삭제. 없어도 성공 (멱등).
pub async fn delete<'e>(ex: impl PgExecutor<'e>, token: &str) -> anyhow::Result<()> {
    if !is_well_formed(token) {
        return Ok(());
    }
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(token_hash(token))
        .execute(ex)
        .await?;
    Ok(())
}

/// 만료된 세션 정리. 지운 개수를 돌려준다.
pub async fn delete_expired(pool: &PgPool) -> anyhow::Result<u64> {
    Ok(
        sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_format() {
        let t = new_token();
        assert!(is_well_formed(&t));
        assert_ne!(t, new_token());
        assert!(!is_well_formed(&t.to_uppercase()));
        assert!(!is_well_formed(&t[..63]));
        assert!(!is_well_formed(&"g".repeat(64)));
        assert_eq!(token_hash(&t).len(), 64);
        assert_ne!(token_hash(&t), t);
    }
}

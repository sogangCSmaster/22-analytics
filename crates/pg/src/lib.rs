//! PostgreSQL 메타데이터 계층.

pub mod ad_spend;
pub mod api_keys;
pub mod attribution;
pub mod attribution_settings;
pub mod audit;
pub mod channel_rules;
pub mod orgs;
pub mod projects;
pub mod seed;
pub mod sessions;
pub mod users;

use anyhow::Context;
pub use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(database_url)
        .await
        .context("Postgres 연결 실패")
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("../../migrations/postgres")
        .run(pool)
        .await
        .context("Postgres 마이그레이션 실패")
}

/// 공개 키 생성: `pk_` + 영숫자 24자.
pub fn new_public_key() -> String {
    format!("pk_{}", random_alnum(24))
}

/// 회원 해시 재해싱용 salt (32바이트 hex).
pub fn new_salt_hex() -> String {
    let bytes: [u8; 32] = rand::random();
    hex::encode(bytes)
}

fn random_alnum(n: usize) -> String {
    use rand::Rng;
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut rng = rand::rng();
    (0..n)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_size_from_env_value() {
        assert_eq!(max_connections(None, 4), 4);
        assert_eq!(max_connections(Some(""), 4), 4);
        assert_eq!(max_connections(Some("abc"), 3), 3);
        assert_eq!(max_connections(Some("0"), 3), 3, "0이면 기본값");
        assert_eq!(max_connections(Some(" 12 "), 3), 12);
        assert_eq!(ENV_MAX_CONNECTIONS, "PG_MAX_CONNECTIONS");
    }
}

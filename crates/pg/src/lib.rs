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

/// 풀 크기 환경 변수. 서버마다 기본값이 다르다 (api 4, ingest 3, jobs 3).
pub const ENV_MAX_CONNECTIONS: &str = "PG_MAX_CONNECTIONS";

/// [`ENV_MAX_CONNECTIONS`] 값 → 풀 크기. 없거나 숫자가 아니거나 0이면 `default`.
pub fn max_connections(value: Option<&str>, default: u32) -> u32 {
    value
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

/// 환경 변수 [`ENV_MAX_CONNECTIONS`]로 정한 풀 크기 (없으면 `default`).
pub fn max_connections_from_env(default: u32) -> u32 {
    max_connections(std::env::var(ENV_MAX_CONNECTIONS).ok().as_deref(), default)
}

/// 연결 풀. 서버 여러 대가 같은 Postgres를 쓰므로 풀은 작게 둔다 (쿼리가 짧고 결과를 캐시한다).
pub async fn connect(database_url: &str, max_connections: u32) -> anyhow::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(max_connections)
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

//! ClickHouse 접근 계층.
//!
//! 모든 조회는 [`ProjectScope`]를 받아 `project_id` 조건을 빠뜨릴 수 없게 한다 (테넌트 격리).

pub mod attribution;
pub mod migrate;
pub mod orders;
pub mod reports;
pub mod rows;

use anyhow::Context;
pub use clickhouse::Client;

/// [`ChConfig::client`] 쿼리 실행 시간 제한 (초).
pub const MAX_EXECUTION_SECS: u32 = 10;

/// 환경 변수 값, 없거나 UTF-8이 아니면 `default`. 서버 설정 로딩(`from_env`) 공용.
pub fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

#[derive(Debug, Clone)]
pub struct ChConfig {
    pub url: String,
    pub user: String,
    pub password: String,
    pub database: String,
}

impl ChConfig {
    /// `CLICKHOUSE_URL`, `CLICKHOUSE_USER`, `CLICKHOUSE_PASSWORD`, `CLICKHOUSE_DATABASE`
    pub fn from_env() -> Self {
        Self {
            url: env_or("CLICKHOUSE_URL", "http://localhost:8123"),
            user: env_or("CLICKHOUSE_USER", "default"),
            password: env_or("CLICKHOUSE_PASSWORD", ""),
            database: env_or("CLICKHOUSE_DATABASE", "analytics"),
        }
    }

    fn base(&self) -> Client {
        Client::default()
            .with_url(&self.url)
            .with_user(&self.user)
            .with_password(&self.password)
    }

    /// 데이터베이스가 지정된 클라이언트.
    ///
    /// 쿼리 실행 시간은 [`MAX_EXECUTION_SECS`]초로 제한한다 (대시보드 폭주 쿼리 차단).
    /// 긴 작업(마이그레이션 백필 등)은 `with_setting("max_execution_time", "0")`으로 푼다.
    pub fn client(&self) -> Client {
        self.base()
            .with_database(&self.database)
            .with_setting("max_execution_time", MAX_EXECUTION_SECS.to_string())
    }

    /// 데이터베이스 지정 없는 클라이언트 (DB 생성용).
    pub fn server_client(&self) -> Client {
        self.base()
    }
}

/// 테넌트 범위. API 계층에서 사용자 권한을 확인한 뒤에만 만든다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectScope {
    project_id: u32,
}

impl ProjectScope {
    /// 호출자는 사용자가 이 프로젝트에 접근할 수 있음을 이미 확인했어야 한다.
    pub fn authorized(project_id: u32) -> Self {
        Self { project_id }
    }

    pub fn project_id(&self) -> u32 {
        self.project_id
    }
}

pub async fn ping(client: &Client) -> anyhow::Result<()> {
    let one: u8 = client
        .query("SELECT 1")
        .fetch_one()
        .await
        .context("ClickHouse 연결 실패")?;
    anyhow::ensure!(one == 1, "ClickHouse 응답 이상");
    Ok(())
}

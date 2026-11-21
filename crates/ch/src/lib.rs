//! ClickHouse 접근 계층.
//!
//! 모든 조회는 [`ProjectScope`]를 받아 `project_id` 조건을 빠뜨릴 수 없게 한다 (테넌트 격리).

pub mod migrate;
pub mod rows;

use anyhow::Context;
pub use clickhouse::Client;

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
        let get = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
        Self {
            url: get("CLICKHOUSE_URL", "http://localhost:8123"),
            user: get("CLICKHOUSE_USER", "default"),
            password: get("CLICKHOUSE_PASSWORD", ""),
            database: get("CLICKHOUSE_DATABASE", "analytics"),
        }
    }

    fn base(&self) -> Client {
        Client::default()
            .with_url(&self.url)
            .with_user(&self.user)
            .with_password(&self.password)
    }

    /// 데이터베이스가 지정된 클라이언트.
    pub fn client(&self) -> Client {
        self.base().with_database(&self.database)
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

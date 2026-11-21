//! 통합 테스트용 격리 DB.
//!
//! ```ignore
//! let db = analytics_testkit::TestDb::new().await;
//! // db.pg: 마이그레이션이 적용된 새 Postgres 데이터베이스
//! // db.ch: 마이그레이션이 적용된 새 ClickHouse 데이터베이스
//! ```
//!
//! 테스트마다 `test_<uuid>` 이름으로 새 데이터베이스를 만들고, 값이 drop되면 지운다.
//! 병렬 테스트와 여러 worktree가 같은 로컬 DB 서버를 써도 서로 간섭하지 않는다.
//!
//! 접속 정보: `TEST_DATABASE_URL`(없으면 `DATABASE_URL`, 기본 로컬), `CLICKHOUSE_URL`.

use analytics_ch::ChConfig;
use sqlx::postgres::PgPoolOptions;

pub struct TestDb {
    pub pg: sqlx::PgPool,
    pub ch: analytics_ch::Client,
    pub ch_config: ChConfig,
    pub pg_url: String,
    name: String,
    admin_url: String,
}

fn admin_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| "postgres://analytics:analytics@localhost:5432/analytics".into())
}

fn replace_db(url: &str, db: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (url, None),
    };
    let base = match base.rfind('/') {
        Some(i) if i > "postgres://".len() => &base[..i],
        _ => base,
    };
    match query {
        Some(q) => format!("{base}/{db}?{q}"),
        None => format!("{base}/{db}"),
    }
}

impl TestDb {
    /// 새 격리 DB를 만들고 두 DB에 마이그레이션을 적용한다.
    /// DB 서버가 없으면 패닉한다 (`make up` 필요).
    pub async fn new() -> Self {
        Self::try_new()
            .await
            .expect("테스트 DB 생성 실패: `make up`으로 DB를 띄웠는지 확인하세요")
    }

    pub async fn try_new() -> anyhow::Result<Self> {
        let name = format!("test_{}", uuid::Uuid::new_v4().simple());
        let admin_url = admin_url();

        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await?;
        let sql = format!("CREATE DATABASE \"{name}\"");
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&admin)
            .await?;
        admin.close().await;

        let pg_url = replace_db(&admin_url, &name);
        let pg = PgPoolOptions::new()
            .max_connections(5)
            .connect(&pg_url)
            .await?;
        analytics_pg::migrate(&pg).await?;

        let mut ch_config = ChConfig::from_env();
        ch_config.database = name.clone();
        analytics_ch::migrate::run(&ch_config).await?;
        let ch = ch_config
            .client()
            // 테스트에서는 insert 직후 조회가 보이도록 한다.
            .with_setting("insert_deduplicate", "0")
            .with_setting("wait_end_of_query", "1");

        Ok(Self {
            pg,
            ch,
            ch_config,
            pg_url,
            name,
            admin_url,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        if std::env::var("TESTKIT_KEEP_DB").is_ok() {
            eprintln!("TESTKIT_KEEP_DB: {} 유지", self.name);
            return;
        }
        let name = self.name.clone();
        let admin_url = self.admin_url.clone();
        let ch_config = self.ch_config.clone();
        let pg = self.pg.clone();
        // 비동기 정리를 별도 스레드의 런타임에서 실행한다 (Drop은 동기).
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let Ok(rt) = rt else { return };
            rt.block_on(async move {
                pg.close().await;
                if let Ok(admin) = PgPoolOptions::new()
                    .max_connections(1)
                    .connect(&admin_url)
                    .await
                {
                    let sql = format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)");
                    let _ = sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                        .execute(&admin)
                        .await;
                }
                let _ = ch_config
                    .server_client()
                    .query(&format!("DROP DATABASE IF EXISTS `{name}`"))
                    .execute()
                    .await;
            });
        })
        .join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_database_name() {
        assert_eq!(
            replace_db("postgres://u:p@h:5432/analytics", "t1"),
            "postgres://u:p@h:5432/t1"
        );
        assert_eq!(
            replace_db("postgres://u:p@h:5432/analytics?sslmode=disable", "t1"),
            "postgres://u:p@h:5432/t1?sslmode=disable"
        );
    }

    #[tokio::test]
    async fn creates_isolated_databases() {
        let a = TestDb::new().await;
        let b = TestDb::new().await;
        assert_ne!(a.name(), b.name());
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM projects")
            .fetch_one(&a.pg)
            .await
            .unwrap();
        assert_eq!(n, 0);
        let c: u64 =
            a.ch.query("SELECT count() FROM events")
                .fetch_one()
                .await
                .unwrap();
        assert_eq!(c, 0);
        let name = a.name().to_owned();
        drop(a);
        let exists: u64 = b
            .ch_config
            .server_client()
            .query("SELECT count() FROM system.databases WHERE name = ?")
            .bind(&name)
            .fetch_one()
            .await
            .unwrap();
        assert_eq!(exists, 0, "drop 후 ClickHouse DB가 지워져야 한다");
    }
}

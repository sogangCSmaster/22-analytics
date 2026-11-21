use anyhow::Context;
use std::path::PathBuf;
use std::time::Duration;

/// ClickHouse 배치 플러시 주기 기본값. 길수록 INSERT(파트) 수가 줄어든다.
pub const DEFAULT_FLUSH_INTERVAL_MS: &str = "3000";

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub database_url: String,
    pub edge_secret: Vec<u8>,
    pub edge_emulation: bool,
    pub archive_dir: PathBuf,
    pub archive_rotate: Duration,
    pub flush_interval: Duration,
    pub flush_rows: usize,
    pub tracker_dist: PathBuf,
    pub dev_site_dir: PathBuf,
    /// 주문을 쓰는 ingest가 여러 대인가 (`ORDERS_MULTI_WRITER`, 기본 false).
    /// 켜면 주문 투영을 쓴 뒤 다시 읽어 프로세스 간 경합을 보정한다 (orders 모듈 문서 5번).
    pub orders_multi_writer: bool,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let get = analytics_ch::env_or;
        let edge_emulation = flag(&get("EDGE_EMULATION", "false"));
        let edge_secret = std::env::var("EDGE_SHARED_SECRET").unwrap_or_default();
        if !edge_emulation && edge_secret.len() < 32 {
            anyhow::bail!("EDGE_SHARED_SECRET은 32자 이상이어야 합니다 (운영 모드)");
        }
        Ok(Self {
            bind: get("INGEST_BIND", "0.0.0.0:8081"),
            database_url: get(
                "DATABASE_URL",
                "postgres://analytics:analytics@localhost:5432/analytics",
            ),
            edge_secret: edge_secret.into_bytes(),
            edge_emulation,
            archive_dir: get("ARCHIVE_DIR", "./var/archive").into(),
            archive_rotate: Duration::from_secs(
                get("ARCHIVE_ROTATE_SECS", "300")
                    .parse()
                    .context("ARCHIVE_ROTATE_SECS")?,
            ),
            flush_interval: Duration::from_millis(
                get("FLUSH_INTERVAL_MS", DEFAULT_FLUSH_INTERVAL_MS)
                    .parse()
                    .context("FLUSH_INTERVAL_MS")?,
            ),
            flush_rows: get("FLUSH_ROWS", "10000").parse().context("FLUSH_ROWS")?,
            tracker_dist: get("TRACKER_DIST", "./tracker/dist").into(),
            dev_site_dir: get("DEV_SITE_DIR", "./dev/site").into(),
            orders_multi_writer: flag(&get("ORDERS_MULTI_WRITER", "false")),
        })
    }
}

/// `1`·`true`·`yes`면 켜짐.
fn flag(v: &str) -> bool {
    matches!(v, "1" | "true" | "yes")
}

#[cfg(test)]
impl Config {
    /// 테스트용: 서명 비밀과 에뮬레이션 여부만 정하고 나머지는 기본값.
    pub fn for_tests(edge_secret: &[u8], edge_emulation: bool) -> Self {
        Self {
            bind: "127.0.0.1:0".into(),
            database_url: String::new(),
            edge_secret: edge_secret.to_vec(),
            edge_emulation,
            archive_dir: "./var/archive".into(),
            archive_rotate: Duration::from_secs(300),
            flush_interval: Duration::from_millis(3000),
            flush_rows: 10_000,
            tracker_dist: "./tracker/dist".into(),
            dev_site_dir: "./dev/site".into(),
            orders_multi_writer: false,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pg_pool_default_is_3() {
        // ingest의 PG 쿼리는 키 검증·프로젝트 조회뿐이고 결과를 캐시한다
        assert_eq!(super::DEFAULT_PG_MAX_CONNECTIONS, 3);
        assert_eq!(super::Config::for_tests(b"", true).pg_max_connections, 3);
    }

    #[test]
    fn default_flush_interval_is_3s() {
        // C6: 초당 INSERT를 1/3로 줄인다 (작은 파트·머지 비용 감소). 지연은 수 초로 충분하다.
        assert_eq!(super::DEFAULT_FLUSH_INTERVAL_MS, "3000");
    }
}

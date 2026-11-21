use anyhow::Context;
use std::path::PathBuf;
use std::time::Duration;

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
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let get = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
        let edge_emulation = matches!(
            get("EDGE_EMULATION", "false").as_str(),
            "1" | "true" | "yes"
        );
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
                get("FLUSH_INTERVAL_MS", "1000")
                    .parse()
                    .context("FLUSH_INTERVAL_MS")?,
            ),
            flush_rows: get("FLUSH_ROWS", "10000").parse().context("FLUSH_ROWS")?,
            tracker_dist: get("TRACKER_DIST", "./tracker/dist").into(),
            dev_site_dir: get("DEV_SITE_DIR", "./dev/site").into(),
        })
    }
}

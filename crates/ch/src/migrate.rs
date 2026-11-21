//! ClickHouse 스키마 마이그레이션. SQL 파일은 바이너리에 포함된다.

use crate::ChConfig;
use anyhow::Context;

/// (버전, SQL). 새 파일을 추가하면 여기에도 추가한다. 적용된 파일은 수정하지 않는다.
/// 버전 순으로 정렬해 둔다 (0002·0003은 다른 작업이 예약).
pub const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_init",
        include_str!("../../../migrations/clickhouse/0001_init.sql"),
    ),
    (
        "0004_rollups",
        include_str!("../../../migrations/clickhouse/0004_rollups.sql"),
    ),
    (
        "0005_storage",
        include_str!("../../../migrations/clickhouse/0005_storage.sql"),
    ),
];

pub async fn run(cfg: &ChConfig) -> anyhow::Result<Vec<&'static str>> {
    let server = cfg.server_client();
    server
        .query(&format!("CREATE DATABASE IF NOT EXISTS `{}`", cfg.database))
        .execute()
        .await
        .context("데이터베이스 생성 실패")?;

    // 백필(INSERT ... SELECT)은 리포트용 실행 시간 제한을 받지 않는다.
    let client = cfg.client().with_setting("max_execution_time", "0");
    client
        .query(
            "CREATE TABLE IF NOT EXISTS schema_migrations \
             (version String, applied_at DateTime DEFAULT now()) \
             ENGINE = ReplacingMergeTree ORDER BY version",
        )
        .execute()
        .await?;

    let applied: Vec<String> = client
        .query("SELECT version FROM schema_migrations FINAL")
        .fetch_all()
        .await?;

    let mut newly = Vec::new();
    for (version, sql) in MIGRATIONS {
        if applied.iter().any(|v| v == version) {
            continue;
        }
        for stmt in statements(sql) {
            client
                .query(&stmt)
                .execute()
                .await
                .with_context(|| format!("마이그레이션 {version} 실패:\n{stmt}"))?;
        }
        client
            .query("INSERT INTO schema_migrations (version) VALUES (?)")
            .bind(*version)
            .execute()
            .await?;
        tracing::info!(version, "ClickHouse 마이그레이션 적용");
        newly.push(*version);
    }
    Ok(newly)
}

/// 주석 줄을 빼고 줄 끝 세미콜론으로 문장을 나눈다.
pub fn statements(sql: &str) -> Vec<String> {
    let cleaned: String = sql
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    cleaned
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.replace('?', "??"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_init_migration() {
        let stmts = statements(MIGRATIONS[0].1);
        assert!(stmts.len() >= 10);
        assert!(stmts.iter().all(|s| s.starts_with("CREATE")));
    }

    #[test]
    fn migrations_are_sorted_and_unique() {
        let versions: Vec<&str> = MIGRATIONS.iter().map(|(v, _)| *v).collect();
        let mut sorted = versions.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(versions, sorted);
    }

    #[test]
    fn splits_rollups_migration() {
        let (_, sql) = MIGRATIONS
            .iter()
            .find(|(v, _)| *v == "0004_rollups")
            .expect("0004");
        let stmts = statements(sql);
        assert!(
            stmts
                .iter()
                .any(|s| s.starts_with("INSERT INTO daily_channel"))
        );
        assert!(
            stmts
                .iter()
                .any(|s| s == "DROP TABLE IF EXISTS daily_stats")
        );
        assert!(stmts.iter().all(|s| !s.contains("??")));
    }
}

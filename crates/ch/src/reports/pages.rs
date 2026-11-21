//! 페이지별 지표.

use super::{DateRange, clamp_limit};
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::PageRow;
use clickhouse::Row;
use serde::Deserialize;

#[derive(Row, Deserialize)]
struct Raw {
    path: String,
    pageviews: u64,
    visitors: u64,
    entrances: u64,
}

/// 페이지뷰 내림차순 상위 `limit`개 (최대 [`MAX_LIMIT`](super::MAX_LIMIT)).
///
/// `path_contains`: 경로 부분 문자열 필터. 글자 그대로 비교한다 (`%`·`_`는 와일드카드가 아님).
pub async fn pages(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
    path_contains: Option<&str>,
    limit: u32,
) -> anyhow::Result<Vec<PageRow>> {
    let filter = path_contains.filter(|s| !s.is_empty());
    let sql = format!(
        "SELECT path,
                sum(pageviews)      AS pageviews,
                uniqMerge(visitors) AS visitors,
                sum(entrances)      AS entrances
         FROM daily_stats
         WHERE project_id = ? AND day BETWEEN toDate(?) AND toDate(?) {path_cond}
         GROUP BY path
         HAVING pageviews > 0
         ORDER BY pageviews DESC, path ASC
         LIMIT ?",
        path_cond = if filter.is_some() {
            "AND position(path, ?) > 0"
        } else {
            ""
        },
    );
    let mut q = client
        .query(&sql)
        .bind(scope.project_id())
        .bind(range.from_ymd())
        .bind(range.to_ymd());
    if let Some(f) = filter {
        q = q.bind(f);
    }
    let rows: Vec<Raw> = q.bind(clamp_limit(limit)).fetch_all().await?;
    Ok(rows
        .into_iter()
        .map(|r| PageRow {
            path: r.path,
            pageviews: r.pageviews,
            visitors: r.visitors,
            entrances: r.entrances,
        })
        .collect())
}

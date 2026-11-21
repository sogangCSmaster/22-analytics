//! 개요: 방문자·세션·페이지뷰·진입·이벤트와 매출, 직전 같은 길이 기간 비교.

use super::DateRange;
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::{OverviewMetrics, OverviewReport};
use clickhouse::Row;
use serde::Deserialize;

#[derive(Row, Deserialize)]
struct TrafficRow {
    visitors: u64,
    sessions: u64,
    pageviews: u64,
    entrances: u64,
    events: u64,
}

#[derive(Row, Deserialize)]
struct RevenueRow {
    revenue: i64,
    orders: u64,
}

pub async fn overview(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
) -> anyhow::Result<OverviewReport> {
    let prev = range.previous();
    Ok(OverviewReport {
        range: range.to_dto(),
        previous_range: prev.to_dto(),
        current: metrics(client, scope, range).await?,
        previous: metrics(client, scope, prev).await?,
    })
}

async fn metrics(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
) -> anyhow::Result<OverviewMetrics> {
    let t: TrafficRow = client
        .query(
            "SELECT uniqMerge(visitors) AS visitors,
                    uniqMerge(sessions) AS sessions,
                    sum(pageviews)      AS pageviews,
                    sum(entrances)      AS entrances,
                    sum(events)         AS events
             FROM daily_stats
             WHERE project_id = ? AND day BETWEEN toDate(?) AND toDate(?)",
        )
        .bind(scope.project_id())
        .bind(range.from_ymd())
        .bind(range.to_ymd())
        .fetch_one()
        .await?;
    let r: RevenueRow = client
        .query(&format!(
            "SELECT toInt64(sum(effective_amount)) AS revenue, count() AS orders
             FROM orders FINAL
             WHERE project_id = ? AND {NOT_CANCELLED}
               AND toDate(ordered_at, '{tz}') BETWEEN toDate(?) AND toDate(?)",
            NOT_CANCELLED = super::NOT_CANCELLED,
            tz = super::TZ,
        ))
        .bind(scope.project_id())
        .bind(range.from_ymd())
        .bind(range.to_ymd())
        .fetch_one()
        .await?;
    Ok(OverviewMetrics {
        visitors: t.visitors,
        sessions: t.sessions,
        pageviews: t.pageviews,
        entrances: t.entrances,
        events: t.events,
        revenue: r.revenue,
        orders: r.orders,
    })
}

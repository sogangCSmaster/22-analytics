//! 개요: 방문자·세션·페이지뷰·진입·이벤트와 매출, 직전 같은 길이 기간 비교.

use super::{DateRange, FINAL_SETTINGS, NOT_CANCELLED, TZ, UNIQ_MERGE};
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

/// 현재·직전 기간을 동시에 조회한다 (기간마다 트래픽·매출 2개 → 최대 4개 동시).
pub async fn overview(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
) -> anyhow::Result<OverviewReport> {
    let prev = range.previous();
    let (current, previous) =
        tokio::try_join!(metrics(client, scope, range), metrics(client, scope, prev))?;
    Ok(OverviewReport {
        range: range.to_dto(),
        previous_range: prev.to_dto(),
        current,
        previous,
    })
}

async fn metrics(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
) -> anyhow::Result<OverviewMetrics> {
    let traffic = async {
        client
            .query(&format!(
                "SELECT {UNIQ_MERGE}(visitors) AS visitors,
                        {UNIQ_MERGE}(sessions) AS sessions,
                        sum(pageviews)         AS pageviews,
                        sum(entrances)         AS entrances,
                        sum(events)            AS events
                 FROM daily_channel
                 WHERE project_id = ? AND day BETWEEN toDate(?) AND toDate(?)"
            ))
            .bind(scope.project_id())
            .bind(range.from_ymd())
            .bind(range.to_ymd())
            .fetch_one::<TrafficRow>()
            .await
    };
    let revenue = async {
        client
            .query(&format!(
                "SELECT toInt64(sum(effective_amount)) AS revenue, count() AS orders
                 FROM orders FINAL
                 WHERE project_id = ? AND {NOT_CANCELLED}
                   AND toDate(ordered_at, '{TZ}') BETWEEN toDate(?) AND toDate(?)
                 {FINAL_SETTINGS}"
            ))
            .bind(scope.project_id())
            .bind(range.from_ymd())
            .bind(range.to_ymd())
            .fetch_one::<RevenueRow>()
            .await
    };
    let (t, r) = tokio::try_join!(traffic, revenue)?;
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

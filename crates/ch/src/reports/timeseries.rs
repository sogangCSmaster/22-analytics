//! 일별 추이.

use super::{DateRange, fmt_date};
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::{TimeseriesPoint, TimeseriesReport};
use clickhouse::Row;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Row, Deserialize)]
struct DayRow {
    date: String,
    visitors: u64,
    sessions: u64,
    pageviews: u64,
}

/// 일별 방문자·세션·페이지뷰. 데이터가 없는 날은 0으로 채운다.
pub async fn timeseries(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
) -> anyhow::Result<TimeseriesReport> {
    let rows: Vec<DayRow> = client
        .query(
            "SELECT toString(day)       AS date,
                    uniqMerge(visitors) AS visitors,
                    uniqMerge(sessions) AS sessions,
                    sum(pageviews)      AS pageviews
             FROM daily_stats
             WHERE project_id = ? AND day BETWEEN toDate(?) AND toDate(?)
             GROUP BY day",
        )
        .bind(scope.project_id())
        .bind(range.from_ymd())
        .bind(range.to_ymd())
        .fetch_all()
        .await?;
    let mut by_day: HashMap<String, DayRow> =
        rows.into_iter().map(|r| (r.date.clone(), r)).collect();
    let points = range
        .dates()
        .map(|d| {
            let date = fmt_date(d);
            match by_day.remove(&date) {
                Some(r) => TimeseriesPoint {
                    date,
                    visitors: r.visitors,
                    sessions: r.sessions,
                    pageviews: r.pageviews,
                },
                None => TimeseriesPoint {
                    date,
                    visitors: 0,
                    sessions: 0,
                    pageviews: 0,
                },
            }
        })
        .collect();
    Ok(TimeseriesReport {
        range: range.to_dto(),
        points,
    })
}

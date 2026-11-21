//! 이벤트 이름별 집계(`daily_events` 롤업)와 속성 값 분포. 속성은 롤업에 없으므로 `events`를 직접
//! 읽는다 (정렬키 `(project_id, toDate(ts, 'Asia/Seoul'), event_name, ...)`라 스캔 범위가 작다).

use super::{DateRange, TZ, UNIQ, UNIQ_MERGE, clamp_limit};
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::{EventCountRow, PropertyValueRow};
use clickhouse::Row;
use serde::Deserialize;

#[derive(Row, Deserialize)]
struct NameRow {
    event_name: String,
    count: u64,
    visitors: u64,
}

#[derive(Row, Deserialize)]
struct ValueRow {
    value: String,
    count: u64,
    visitors: u64,
}

/// 이벤트 이름별 발생 수와 순 방문자. 발생 수 내림차순.
pub async fn events(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
    limit: u32,
) -> anyhow::Result<Vec<EventCountRow>> {
    let rows: Vec<NameRow> = client
        .query(&format!(
            "SELECT toString(event_name)    AS event_name,
                    sum(events)             AS count,
                    {UNIQ_MERGE}(visitors)  AS visitors
             FROM daily_events
             WHERE project_id = ? AND day BETWEEN toDate(?) AND toDate(?)
             GROUP BY event_name
             ORDER BY count DESC, event_name ASC
             LIMIT ?"
        ))
        .bind(scope.project_id())
        .bind(range.from_ymd())
        .bind(range.to_ymd())
        .bind(clamp_limit(limit))
        .fetch_all()
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| EventCountRow {
            event_name: r.event_name,
            count: r.count,
            visitors: r.visitors,
        })
        .collect())
}

/// 한 이벤트의 속성 `prop_key` 값 분포. 그 속성이 있는 이벤트만 센다. 발생 수 내림차순.
pub async fn event_properties(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
    event_name: &str,
    prop_key: &str,
    limit: u32,
) -> anyhow::Result<Vec<PropertyValueRow>> {
    let rows: Vec<ValueRow> = client
        .query(&format!(
            "SELECT `props.value`[indexOf(`props.key`, ?)] AS value,
                    count()                                AS count,
                    {UNIQ}(visitor_id)                     AS visitors
             FROM events
             WHERE project_id = ? AND toDate(ts, '{TZ}') BETWEEN toDate(?) AND toDate(?)
               AND event_name = ? AND has(`props.key`, ?)
             GROUP BY value
             ORDER BY count DESC, value ASC
             LIMIT ?"
        ))
        .bind(prop_key)
        .bind(scope.project_id())
        .bind(range.from_ymd())
        .bind(range.to_ymd())
        .bind(event_name)
        .bind(prop_key)
        .bind(clamp_limit(limit))
        .fetch_all()
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| PropertyValueRow {
            value: r.value,
            count: r.count,
            visitors: r.visitors,
        })
        .collect())
}

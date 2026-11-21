//! 실시간 (최근 30분). 롤업을 거치지 않고 `events`를 직접 읽는다.
//!
//! 창은 모두 `(now − N분, now]`이다. 분당 페이지뷰는 `now`가 속한 분을 마지막으로 하는 30개 구간이다.

use crate::{Client, ProjectScope};
use analytics_core::dto::reports::{RealtimeMinute, RealtimePage, RealtimeReport};
use clickhouse::Row;
use serde::Deserialize;
use std::collections::HashMap;
use time::{
    Duration, OffsetDateTime, UtcOffset,
    macros::{format_description, offset},
};

/// 실시간 상위 페이지 수.
pub const REALTIME_TOP_PAGES: u32 = 10;
const MINUTES: i64 = 30;
const MINUTE_MS: i64 = 60_000;
const KST: UtcOffset = offset!(+9);

#[derive(Row, Deserialize)]
struct ActiveRow {
    active_5m: u64,
    active_30m: u64,
}

#[derive(Row, Deserialize)]
struct BucketRow {
    bucket: i64,
    pageviews: u64,
}

#[derive(Row, Deserialize)]
struct PageRow {
    path: String,
    pageviews: u64,
}

pub async fn realtime(
    client: &Client,
    scope: ProjectScope,
    now: OffsetDateTime,
) -> anyhow::Result<RealtimeReport> {
    let now_ms = (now.unix_timestamp_nanos() / 1_000_000) as i64;
    let p = scope.project_id();

    let active: ActiveRow = client
        .query(
            "SELECT uniqIf(visitor_id, ts > fromUnixTimestamp64Milli(toInt64(?), 'UTC')) AS active_5m,
                    uniq(visitor_id) AS active_30m
             FROM events
             WHERE project_id = ?
               AND ts > fromUnixTimestamp64Milli(toInt64(?), 'UTC')
               AND ts <= fromUnixTimestamp64Milli(toInt64(?), 'UTC')",
        )
        .bind(now_ms - 5 * MINUTE_MS)
        .bind(p)
        .bind(now_ms - MINUTES * MINUTE_MS)
        .bind(now_ms)
        .fetch_one()
        .await?;

    // 첫 구간 시작: now가 속한 분의 시작 − 29분
    let first_ms = now_ms.div_euclid(MINUTE_MS) * MINUTE_MS - (MINUTES - 1) * MINUTE_MS;
    let buckets: Vec<BucketRow> = client
        .query(
            "SELECT toInt64(intDiv(toUnixTimestamp64Milli(ts) - toInt64(?), 60000)) AS bucket,
                    count() AS pageviews
             FROM events
             WHERE project_id = ? AND event_name = 'page_view'
               AND ts >= fromUnixTimestamp64Milli(toInt64(?), 'UTC')
               AND ts <= fromUnixTimestamp64Milli(toInt64(?), 'UTC')
             GROUP BY bucket",
        )
        .bind(first_ms)
        .bind(p)
        .bind(first_ms)
        .bind(now_ms)
        .fetch_all()
        .await?;
    let counts: HashMap<i64, u64> = buckets
        .into_iter()
        .map(|b| (b.bucket, b.pageviews))
        .collect();
    let hm = format_description!("[hour]:[minute]");
    let per_minute = (0..MINUTES)
        .map(|i| {
            let start =
                OffsetDateTime::UNIX_EPOCH + Duration::milliseconds(first_ms + i * MINUTE_MS);
            RealtimeMinute {
                minute: start.to_offset(KST).format(hm).unwrap_or_default(),
                pageviews: counts.get(&i).copied().unwrap_or(0),
            }
        })
        .collect();

    let pages: Vec<PageRow> = client
        .query(
            "SELECT path, count() AS pageviews
             FROM events
             WHERE project_id = ? AND event_name = 'page_view'
               AND ts > fromUnixTimestamp64Milli(toInt64(?), 'UTC')
               AND ts <= fromUnixTimestamp64Milli(toInt64(?), 'UTC')
             GROUP BY path
             ORDER BY pageviews DESC, path ASC
             LIMIT ?",
        )
        .bind(p)
        .bind(now_ms - MINUTES * MINUTE_MS)
        .bind(now_ms)
        .bind(REALTIME_TOP_PAGES)
        .fetch_all()
        .await?;

    let iso = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]+09:00");
    Ok(RealtimeReport {
        now: now.to_offset(KST).format(iso).unwrap_or_default(),
        active_5m: active.active_5m,
        active_30m: active.active_30m,
        per_minute,
        top_pages: pages
            .into_iter()
            .map(|r| RealtimePage {
                path: r.path,
                pageviews: r.pageviews,
            })
            .collect(),
    })
}

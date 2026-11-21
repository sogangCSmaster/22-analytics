//! 실시간 (최근 30분). 롤업을 거치지 않고 `events`를 직접 읽는다. 세 쿼리는 동시에 보낸다.
//!
//! 시각 조건만으로는 정렬키(`project_id, toDate(ts, 'Asia/Seoul'), ...`)를 못 쓰므로 창이 걸친
//! KST 일자 조건을 함께 넣어 프로젝트의 오늘(자정 직후면 어제까지) 그래뉼만 읽게 한다.
//!
//! 창은 모두 `(now − N분, now]`이다. 분당 페이지뷰는 `now`가 속한 분을 마지막으로 하는 30개 구간이다.

use super::{UNIQ, fmt_date};
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::{RealtimeMinute, RealtimePage, RealtimeReport};
use analytics_core::timeutil::{KST, dt_to_ms, kst_date};
use clickhouse::Row;
use serde::Deserialize;
use std::collections::HashMap;
use time::{Duration, OffsetDateTime, macros::format_description};

/// 실시간 상위 페이지 수.
pub const REALTIME_TOP_PAGES: u32 = 10;
const MINUTES: i64 = 30;
const MINUTE_MS: i64 = 60_000;

/// SQL 조각: 정렬키 일자 조건 (바인딩 2개: 시작일, 종료일).
const DAY_COND: &str = "toDate(ts, 'Asia/Seoul') BETWEEN toDate(?) AND toDate(?)";

/// Unix ms → Asia/Seoul 일자 `YYYY-MM-DD`.
fn kst_ymd(ms: i64) -> String {
    fmt_date(kst_date(ms))
}

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
    let now_ms = dt_to_ms(now);
    let p = scope.project_id();
    let since_30m = now_ms - MINUTES * MINUTE_MS;
    // 첫 구간 시작: now가 속한 분의 시작 − 29분 (30분 창 시작보다 늦거나 같다)
    let first_ms = now_ms.div_euclid(MINUTE_MS) * MINUTE_MS - (MINUTES - 1) * MINUTE_MS;
    // 정렬키 `toDate(ts, 'Asia/Seoul')` 조건: 창이 자정을 넘으면 이틀이 된다.
    let day_from = kst_ymd(since_30m);
    let day_to = kst_ymd(now_ms);

    let active = async {
        client
            .query(&format!(
                "SELECT uniqCombinedIf(14)(visitor_id, ts > fromUnixTimestamp64Milli(toInt64(?), 'UTC')) AS active_5m,
                        {UNIQ}(visitor_id) AS active_30m
                 FROM events
                 WHERE project_id = ? AND {DAY_COND}
                   AND ts > fromUnixTimestamp64Milli(toInt64(?), 'UTC')
                   AND ts <= fromUnixTimestamp64Milli(toInt64(?), 'UTC')"
            ))
            .bind(now_ms - 5 * MINUTE_MS)
            .bind(p)
            .bind(&day_from)
            .bind(&day_to)
            .bind(since_30m)
            .bind(now_ms)
            .fetch_one::<ActiveRow>()
            .await
    };

    let buckets = async {
        client
            .query(&format!(
                "SELECT toInt64(intDiv(toUnixTimestamp64Milli(ts) - toInt64(?), 60000)) AS bucket,
                        count() AS pageviews
                 FROM events
                 WHERE project_id = ? AND {DAY_COND} AND event_name = 'page_view'
                   AND ts >= fromUnixTimestamp64Milli(toInt64(?), 'UTC')
                   AND ts <= fromUnixTimestamp64Milli(toInt64(?), 'UTC')
                 GROUP BY bucket"
            ))
            .bind(first_ms)
            .bind(p)
            .bind(&day_from)
            .bind(&day_to)
            .bind(first_ms)
            .bind(now_ms)
            .fetch_all::<BucketRow>()
            .await
    };

    let pages = async {
        client
            .query(&format!(
                "SELECT path, count() AS pageviews
                 FROM events
                 WHERE project_id = ? AND {DAY_COND} AND event_name = 'page_view'
                   AND ts > fromUnixTimestamp64Milli(toInt64(?), 'UTC')
                   AND ts <= fromUnixTimestamp64Milli(toInt64(?), 'UTC')
                 GROUP BY path
                 ORDER BY pageviews DESC, path ASC
                 LIMIT ?"
            ))
            .bind(p)
            .bind(&day_from)
            .bind(&day_to)
            .bind(since_30m)
            .bind(now_ms)
            .bind(REALTIME_TOP_PAGES)
            .fetch_all::<PageRow>()
            .await
    };

    let (active, buckets, pages) = tokio::try_join!(active, buckets, pages)?;
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

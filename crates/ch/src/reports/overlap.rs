//! 중복 매체 조합: 주문별 채널 경로의 빈도 (docs/ARCHITECTURE.md §7.3).
//!
//! 접점 목록은 last_click 결과에서 읽는다 (모든 모델이 같은 접점을 갖고, 어시스트 계산도 last_click 기준).
//! 경로는 `touch_index` 순서이고 연속된 같은 채널은 하나로 합친다
//! (naver_sa, naver_sa, meta → naver_sa → meta). 서로 다른 채널이 2개 이상인 주문만 센다.

use super::{DateRange, FINAL_SETTINGS, TZ, clamp_limit};
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::{OverlapPath, OverlapReport};
use clickhouse::Row;
use serde::Deserialize;

#[derive(Row, Deserialize)]
struct Raw {
    seq: Vec<String>,
    orders: u64,
    total: u64,
}

pub async fn overlap_paths(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
    lookback_days: u8,
    limit: u32,
) -> anyhow::Result<OverlapReport> {
    let rows: Vec<Raw> = client
        .query(&format!(
            "SELECT seq, count() AS orders, toUInt64(sum(count()) OVER ()) AS total
             FROM (
                 SELECT order_id,
                        arrayMap(x -> x.2, arraySort(x -> x.1, groupArray((touch_index, toString(channel))))) AS chs,
                        arrayFilter((c, i) -> i = 1 OR c != chs[i - 1], chs, arrayEnumerate(chs)) AS seq
                 FROM attributed_revenue FINAL
                 WHERE project_id = ? AND model = 'last_click' AND lookback_days = ?
                   AND is_deleted = 0 AND touch_index > 0
                   AND toDate(ordered_at, '{TZ}') BETWEEN toDate(?) AND toDate(?)
                 GROUP BY order_id
                 HAVING length(arrayDistinct(chs)) >= 2
             )
             GROUP BY seq
             ORDER BY orders DESC, seq ASC
             LIMIT ?
             {FINAL_SETTINGS}"
        ))
        .bind(scope.project_id())
        .bind(lookback_days)
        .bind(range.from_ymd())
        .bind(range.to_ymd())
        .bind(clamp_limit(limit))
        .fetch_all()
        .await?;
    Ok(OverlapReport {
        range: range.to_dto(),
        lookback_days,
        multi_channel_orders: rows.first().map(|r| r.total).unwrap_or(0),
        paths: rows
            .into_iter()
            .map(|r| OverlapPath {
                channels: r.seq,
                orders: r.orders,
            })
            .collect(),
    })
}

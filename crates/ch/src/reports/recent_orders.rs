//! 최근 주문 목록 (취소 포함, 주문 시각 내림차순).
//!
//! 채널은 last_click 결과의 귀속 접점(`is_assist = 0`, 미귀속 포함)이다. 기여 연산 전이거나
//! 취소되어 삭제 표시된 주문은 `None`.

use super::{DateRange, FINAL_SETTINGS, TZ, clamp_limit, kst_iso};
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::{RecentOrderRow, RecentOrdersReport};
use clickhouse::Row;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Row, Deserialize)]
struct OrderRaw {
    order_id: String,
    ordered_kst: String,
    ordered_at_ms: i64,
    effective_amount: i64,
    status: String,
}

#[derive(Row, Deserialize)]
struct ChannelRaw {
    order_id: String,
    channel: String,
}

#[derive(Row, Deserialize)]
struct Count {
    total: u64,
}

/// 전체 건수와 주문 페이지를 동시에 읽고, 그 페이지 주문의 last_click 채널만 따로 찾는다
/// (기간 전체 기여 결과를 조인하지 않는다).
pub async fn recent_orders(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
    lookback_days: u8,
    limit: u32,
    offset: u64,
) -> anyhow::Result<RecentOrdersReport> {
    let total = async {
        client
            .query(&format!(
                "SELECT count() AS total
                 FROM orders FINAL
                 WHERE project_id = ? AND toDate(ordered_at, '{TZ}') BETWEEN toDate(?) AND toDate(?)
                 {FINAL_SETTINGS}"
            ))
            .bind(scope.project_id())
            .bind(range.from_ymd())
            .bind(range.to_ymd())
            .fetch_one::<Count>()
            .await
    };
    let page = async {
        client
            .query(&format!(
                "SELECT order_id, {ordered_kst} AS ordered_kst,
                        toUnixTimestamp64Milli(ordered_at) AS ordered_at_ms,
                        effective_amount, toString(status) AS status
                 FROM orders FINAL
                 WHERE project_id = ? AND toDate(ordered_at, '{TZ}') BETWEEN toDate(?) AND toDate(?)
                 ORDER BY ordered_at DESC, order_id ASC
                 LIMIT ? OFFSET ?
                 {FINAL_SETTINGS}",
                ordered_kst = kst_iso("ordered_at"),
            ))
            .bind(scope.project_id())
            .bind(range.from_ymd())
            .bind(range.to_ymd())
            .bind(clamp_limit(limit))
            .bind(offset)
            .fetch_all::<OrderRaw>()
            .await
    };
    let (total, orders) = tokio::try_join!(total, page)?;

    let mut channels: HashMap<String, String> = HashMap::new();
    if let (Some(min_ms), Some(max_ms)) = (
        orders.iter().map(|o| o.ordered_at_ms).min(),
        orders.iter().map(|o| o.ordered_at_ms).max(),
    ) {
        let ids: Vec<&str> = orders.iter().map(|o| o.order_id.as_str()).collect();
        // (주문, 접점)마다 최신 결과를 고른 뒤 삭제·어시스트를 거른다 (거른 뒤 고르면 옛 행이 살아난다).
        let rows: Vec<ChannelRaw> = client
            .query(
                "SELECT order_id, argMax(toString(t.1), touch_index) AS channel
                 FROM (
                     SELECT order_id, touch_index,
                            argMax(tuple(channel, is_assist, is_deleted), computed_at) AS t
                     FROM attributed_revenue
                     WHERE project_id = ? AND model = 'last_click' AND lookback_days = ?
                       AND order_id IN ?
                       AND ordered_at BETWEEN fromUnixTimestamp64Milli(toInt64(?), 'UTC')
                                          AND fromUnixTimestamp64Milli(toInt64(?), 'UTC')
                     GROUP BY order_id, touch_index
                     HAVING t.2 = 0 AND t.3 = 0
                 )
                 GROUP BY order_id",
            )
            .bind(scope.project_id())
            .bind(lookback_days)
            .bind(&ids)
            .bind(min_ms)
            .bind(max_ms)
            .fetch_all()
            .await?;
        channels = rows.into_iter().map(|r| (r.order_id, r.channel)).collect();
    }

    Ok(RecentOrdersReport {
        range: range.to_dto(),
        total: total.total,
        rows: orders
            .into_iter()
            .map(|o| RecentOrderRow {
                last_click_channel: channels.remove(&o.order_id),
                order_id: o.order_id,
                ordered_at: o.ordered_kst,
                effective_amount: o.effective_amount,
                status: o.status,
            })
            .collect(),
    })
}

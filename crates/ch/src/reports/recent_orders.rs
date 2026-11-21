//! 최근 주문 목록 (취소 포함, 주문 시각 내림차순).
//!
//! 채널은 last_click 결과의 귀속 접점(`is_assist = 0`, 미귀속 포함)이다. 기여 연산 전이거나
//! 취소되어 삭제 표시된 주문은 `None`.

use super::{DateRange, TZ, clamp_limit, kst_iso};
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::{RecentOrderRow, RecentOrdersReport};
use clickhouse::Row;
use serde::Deserialize;

#[derive(Row, Deserialize)]
struct Raw {
    order_id: String,
    ordered_at: String,
    effective_amount: i64,
    status: String,
    channel: Option<String>,
}

#[derive(Row, Deserialize)]
struct Count {
    total: u64,
}

pub async fn recent_orders(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
    lookback_days: u8,
    limit: u32,
    offset: u64,
) -> anyhow::Result<RecentOrdersReport> {
    let total: Count = client
        .query(&format!(
            "SELECT count() AS total
             FROM orders FINAL
             WHERE project_id = ? AND toDate(ordered_at, '{TZ}') BETWEEN toDate(?) AND toDate(?)"
        ))
        .bind(scope.project_id())
        .bind(range.from_ymd())
        .bind(range.to_ymd())
        .fetch_one()
        .await?;

    let rows: Vec<Raw> = client
        .query(&format!(
            "SELECT o.order_id AS order_id, {ordered_at} AS ordered_at,
                    o.effective_amount AS effective_amount, o.status AS status,
                    a.channel AS channel
             FROM (
                 SELECT order_id, ordered_at, effective_amount, toString(status) AS status
                 FROM orders FINAL
                 WHERE project_id = ? AND toDate(ordered_at, '{TZ}') BETWEEN toDate(?) AND toDate(?)
                 ORDER BY ordered_at DESC, order_id ASC
                 LIMIT ? OFFSET ?
             ) AS o
             LEFT JOIN (
                 SELECT order_id, argMax(toString(channel), touch_index) AS channel
                 FROM attributed_revenue FINAL
                 WHERE project_id = ? AND model = 'last_click' AND lookback_days = ?
                   AND is_deleted = 0 AND is_assist = 0
                   AND toDate(ordered_at, '{TZ}') BETWEEN toDate(?) AND toDate(?)
                 GROUP BY order_id
             ) AS a ON a.order_id = o.order_id
             ORDER BY o.ordered_at DESC, o.order_id ASC
             SETTINGS join_use_nulls = 1",
            ordered_at = kst_iso("o.ordered_at"),
        ))
        .bind(scope.project_id())
        .bind(range.from_ymd())
        .bind(range.to_ymd())
        .bind(clamp_limit(limit))
        .bind(offset)
        .bind(scope.project_id())
        .bind(lookback_days)
        .bind(range.from_ymd())
        .bind(range.to_ymd())
        .fetch_all()
        .await?;

    Ok(RecentOrdersReport {
        range: range.to_dto(),
        total: total.total,
        rows: rows
            .into_iter()
            .map(|r| RecentOrderRow {
                order_id: r.order_id,
                ordered_at: r.ordered_at,
                effective_amount: r.effective_amount,
                status: r.status,
                last_click_channel: r.channel,
            })
            .collect(),
    })
}

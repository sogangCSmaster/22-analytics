//! 실제 매출 (중복 소거) 리포트 (docs/ARCHITECTURE.md §7.3, 매뉴얼 1.D).
//!
//! `attributed_revenue`는 WP2 기여 연산 잡이 쓴다. 재연산·취소는 `computed_at`이 더 큰 행으로
//! 대체되므로 항상 `FINAL`과 `is_deleted = 0`으로 읽는다. 기간은 주문 시각(Asia/Seoul) 기준.
//! 채널별 합계와 주문 수 두 쿼리는 동시에 보낸다.

use super::{DateRange, FINAL_SETTINGS, TZ, ratio};
use crate::{Client, ProjectScope};
use analytics_core::attribution::Model;
use analytics_core::dto::reports::{ChannelRevenueRow, RevenueReport};
use clickhouse::Row;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Row, Deserialize)]
struct Raw {
    channel: String,
    raw_claimed: i64,
    attributed: i64,
    assists: u64,
}

#[derive(Row, Deserialize)]
struct Count {
    orders: u64,
}

/// - `raw_sum`: Σ amount (model = 'raw_sum') — 매체 합산 매출
/// - `deduplicated`: Σ amount (선택 모델) — 실제 매출
/// - 채널별 어시스트는 모델과 무관하게 last_click의 `is_assist = 1` 접점 수
/// - `spend`: 채널 → 광고비 (Postgres `ad_spend`를 API 계층이 합산해 넘긴다).
///   광고비만 있고 매출이 없는 채널도 행으로 나온다 (ROAS 0).
pub async fn revenue_report(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
    model: Model,
    lookback_days: u8,
    spend: &HashMap<String, i64>,
) -> anyhow::Result<RevenueReport> {
    let rows = async {
        client
            .query(&format!(
                "SELECT toString(channel)                                  AS channel,
                        toInt64(sumIf(amount, model = 'raw_sum'))          AS raw_claimed,
                        toInt64(sumIf(amount, model = ?))                  AS attributed,
                        countIf(model = 'last_click' AND is_assist = 1)    AS assists
                 FROM attributed_revenue FINAL
                 WHERE project_id = ? AND lookback_days = ? AND is_deleted = 0
                   AND model IN ('raw_sum', 'last_click', ?)
                   AND toDate(ordered_at, '{TZ}') BETWEEN toDate(?) AND toDate(?)
                 GROUP BY channel
                 {FINAL_SETTINGS}"
            ))
            .bind(model.as_str())
            .bind(scope.project_id())
            .bind(lookback_days)
            .bind(model.as_str())
            .bind(range.from_ymd())
            .bind(range.to_ymd())
            .fetch_all::<Raw>()
            .await
    };

    let orders = async {
        client
            .query(&format!(
                "SELECT uniqExact(order_id) AS orders
                 FROM attributed_revenue FINAL
                 WHERE project_id = ? AND model = ? AND lookback_days = ? AND is_deleted = 0
                   AND toDate(ordered_at, '{TZ}') BETWEEN toDate(?) AND toDate(?)
                 {FINAL_SETTINGS}"
            ))
            .bind(scope.project_id())
            .bind(model.as_str())
            .bind(lookback_days)
            .bind(range.from_ymd())
            .bind(range.to_ymd())
            .fetch_one::<Count>()
            .await
    };

    let (rows, orders) = tokio::try_join!(rows, orders)?;

    Ok(build(
        range,
        model,
        lookback_days,
        rows,
        orders.orders,
        spend,
    ))
}

fn build(
    range: DateRange,
    model: Model,
    lookback_days: u8,
    rows: Vec<Raw>,
    orders: u64,
    spend: &HashMap<String, i64>,
) -> RevenueReport {
    let raw_sum: i64 = rows.iter().map(|r| r.raw_claimed).sum();
    let deduplicated: i64 = rows.iter().map(|r| r.attributed).sum();
    let spend_of = |c: &str| spend.get(c).copied().unwrap_or(0).max(0);
    let roas = |rev: i64, cost: i64| (cost > 0).then(|| rev as f64 / cost as f64);

    let mut channels: Vec<ChannelRevenueRow> = rows
        .iter()
        .map(|r| {
            let s = spend_of(&r.channel);
            ChannelRevenueRow {
                channel: r.channel.clone(),
                raw_claimed: r.raw_claimed,
                attributed: r.attributed,
                share: ratio(r.attributed as f64, deduplicated as f64),
                assists: r.assists,
                spend: s,
                roas: roas(r.attributed, s),
            }
        })
        .collect();
    // 광고비만 있는 채널
    for c in spend.keys() {
        if !channels.iter().any(|r| &r.channel == c) {
            let s = spend_of(c);
            channels.push(ChannelRevenueRow {
                channel: c.clone(),
                raw_claimed: 0,
                attributed: 0,
                share: 0.0,
                assists: 0,
                spend: s,
                roas: roas(0, s),
            });
        }
    }
    channels.sort_by(|a, b| {
        b.attributed
            .cmp(&a.attributed)
            .then(b.raw_claimed.cmp(&a.raw_claimed))
            .then_with(|| a.channel.cmp(&b.channel))
    });

    let spend_total: i64 = channels.iter().map(|c| c.spend).sum();
    let duplicate_amount = (raw_sum - deduplicated).max(0);
    RevenueReport {
        range: range.to_dto(),
        model,
        lookback_days,
        raw_sum,
        deduplicated,
        duplicate_amount,
        duplicate_rate: ratio(duplicate_amount as f64, raw_sum as f64),
        orders,
        spend_total,
        roas: roas(deduplicated, spend_total),
        channels,
    }
}

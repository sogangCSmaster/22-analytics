//! 전자상거래 퍼널. `windowFunnel`로 방문자별 도달 단계를 구한다.
//!
//! 단계는 순서대로 거쳐야 한다: 상품 조회 없이 장바구니에 담은 방문자는 0단계,
//! 구매 후 상품을 조회한 방문자는 1단계다. 창은 조회 기간 전체.

use super::{DateRange, TZ, ratio};
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::{FunnelReport, FunnelStep};
use clickhouse::Row;
use serde::Deserialize;

pub const FUNNEL_STEPS: [&str; 4] = ["view_item", "add_to_cart", "begin_checkout", "purchase"];

#[derive(Row, Deserialize)]
struct Raw {
    s1: u64,
    s2: u64,
    s3: u64,
    s4: u64,
}

pub async fn ecommerce_funnel(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
) -> anyhow::Result<FunnelReport> {
    let window_secs = range.days() as u64 * 86_400;
    let r: Raw = client
        .query(&format!(
            "SELECT countIf(level >= 1) AS s1, countIf(level >= 2) AS s2,
                    countIf(level >= 3) AS s3, countIf(level >= 4) AS s4
             FROM (
                 SELECT visitor_id,
                        windowFunnel(?)(
                            toDateTime(ts),
                            event_name = 'view_item',
                            event_name = 'add_to_cart',
                            event_name = 'begin_checkout',
                            event_name = 'purchase'
                        ) AS level
                 FROM events
                 WHERE project_id = ? AND toDate(ts, '{TZ}') BETWEEN toDate(?) AND toDate(?)
                   AND event_name IN ('view_item', 'add_to_cart', 'begin_checkout', 'purchase')
                 GROUP BY visitor_id
             )"
        ))
        .bind(window_secs)
        .bind(scope.project_id())
        .bind(range.from_ymd())
        .bind(range.to_ymd())
        .fetch_one()
        .await?;
    let counts = [r.s1, r.s2, r.s3, r.s4];
    let first = counts[0] as f64;
    let steps = FUNNEL_STEPS
        .iter()
        .zip(counts)
        .enumerate()
        .map(|(i, (name, n))| {
            let prev = if i == 0 { first } else { counts[i - 1] as f64 };
            FunnelStep {
                event_name: (*name).to_owned(),
                visitors: n,
                rate_from_prev: ratio(n as f64, prev),
                rate_from_first: ratio(n as f64, first),
            }
        })
        .collect();
    Ok(FunnelReport {
        range: range.to_dto(),
        steps,
    })
}

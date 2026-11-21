//! 주문 상세 (드로어): 주문 정보, 접점 타임라인, 모델별 배분.

use super::kst_iso;
use crate::{Client, ProjectScope};
use analytics_core::attribution::Model;
use analytics_core::dto::reports::{ModelShares, OrderDetail, OrderInfo, OrderTouch, ShareRow};
use clickhouse::Row;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Row, Deserialize)]
struct OrderRaw {
    order_id: String,
    source: String,
    status: String,
    currency: String,
    gross_amount: i64,
    net_amount: i64,
    refunded_amount: i64,
    effective_amount: i64,
    ordered_at: String,
}

#[derive(Row, Deserialize)]
struct ShareRaw {
    model: String,
    touch_index: u16,
    channel: String,
    utm_campaign: String,
    touch_ts: Option<String>,
    amount: i64,
    is_assist: u8,
}

/// 주문이 없거나 다른 프로젝트의 주문이면 `None`.
pub async fn order_detail(
    client: &Client,
    scope: ProjectScope,
    order_id: &str,
    lookback_days: u8,
) -> anyhow::Result<Option<OrderDetail>> {
    let order: Option<OrderRaw> = client
        .query(&format!(
            "SELECT order_id, toString(source) AS source, toString(status) AS status,
                    toString(currency) AS currency, gross_amount, net_amount,
                    refunded_amount, effective_amount, {ordered_at} AS ordered_at
             FROM orders FINAL
             WHERE project_id = ? AND order_id = ?
             LIMIT 1",
            ordered_at = kst_iso("ordered_at"),
        ))
        .bind(scope.project_id())
        .bind(order_id)
        .fetch_optional()
        .await?;
    let Some(o) = order else {
        return Ok(None);
    };

    let shares: Vec<ShareRaw> = client
        .query(&format!(
            "SELECT toString(model) AS model, touch_index, toString(channel) AS channel,
                    utm_campaign, {touch_ts} AS touch_ts, amount, is_assist
             FROM attributed_revenue FINAL
             WHERE project_id = ? AND order_id = ? AND lookback_days = ? AND is_deleted = 0
             ORDER BY model, touch_index",
            touch_ts = kst_iso("touch_ts"),
        ))
        .bind(scope.project_id())
        .bind(order_id)
        .bind(lookback_days)
        .fetch_all()
        .await?;

    // 접점은 모델과 무관하게 같다: touch_index로 한 번씩만 모은다 (0 = 미귀속 제외).
    let mut touches: BTreeMap<u16, OrderTouch> = BTreeMap::new();
    let mut by_model: BTreeMap<usize, Vec<ShareRow>> = BTreeMap::new();
    for s in shares {
        if s.touch_index > 0 {
            touches.entry(s.touch_index).or_insert_with(|| OrderTouch {
                touch_index: s.touch_index,
                ts: s.touch_ts.clone().unwrap_or_default(),
                channel: s.channel.clone(),
                campaign: s.utm_campaign.clone(),
            });
        }
        // 알 수 없는 모델 이름은 무시한다 (잡이 새 모델을 먼저 배포한 경우).
        let Some(pos) = Model::ALL.iter().position(|m| m.as_str() == s.model) else {
            continue;
        };
        by_model.entry(pos).or_default().push(ShareRow {
            touch_index: s.touch_index,
            channel: s.channel,
            campaign: s.utm_campaign,
            amount: s.amount,
            is_assist: s.is_assist != 0,
        });
    }

    Ok(Some(OrderDetail {
        order: OrderInfo {
            order_id: o.order_id,
            source: o.source,
            status: o.status,
            currency: o.currency,
            gross_amount: o.gross_amount,
            net_amount: o.net_amount,
            refunded_amount: o.refunded_amount,
            effective_amount: o.effective_amount,
            ordered_at: o.ordered_at,
        },
        lookback_days,
        touches: touches.into_values().collect(),
        shares: by_model
            .into_iter()
            .map(|(pos, rows)| ModelShares {
                model: Model::ALL[pos],
                rows,
            })
            .collect(),
    }))
}

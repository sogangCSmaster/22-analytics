//! 주문 이벤트(`order_events`)와 주문 현재 상태(`orders`) 읽기·쓰기.
//!
//! 다른 패키지와의 계약:
//! - `orders`는 항상 `FINAL`로 읽는다 (ReplacingMergeTree(version)).
//! - 한 주문의 `ordered_at`(파티션 키)은 버전이 바뀌어도 그대로다 (`analytics_core::orders::project`).
//!   그래서 같은 주문의 모든 버전이 한 파티션에 있고, 리포트는 파티션 간 병합 없이 `FINAL`을 쓸 수 있다.
//! - `version`은 unix ms 근사값이며 주문마다 엄격히 증가한다: `max(이전 version + 1, now_ms)`.
//! - 기여 연산 잡은 `version > watermark`로 바뀐 주문을 찾는다 ([`orders_changed_since`]).
//! - `order_events`는 append-only다. 같은 멱등 키의 중복 행이 있을 수 있으므로
//!   읽는 쪽은 [`analytics_core::orders::fold`]처럼 멱등 키로 중복을 제거한다.

use crate::ProjectScope;
use analytics_core::orders::{OrderEvent, OrderEventType, OrderSource, OrderState, OrderStatus};
use analytics_core::timeutil::{dt_to_ms, ms_to_dt};
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Row, Serialize, Deserialize)]
pub struct OrderEventRow {
    pub project_id: u32,
    pub order_id: String,
    pub event_type: String,
    pub source: String,
    pub idempotency_key: String,
    #[serde(with = "clickhouse::serde::uuid::option")]
    pub visitor_id: Option<Uuid>,
    pub member_hash: String,
    pub currency: String,
    pub gross_amount: i64,
    pub net_amount: i64,
    pub refund_amount: i64,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    pub occurred_at: OffsetDateTime,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    pub received_at: OffsetDateTime,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Row, Serialize, Deserialize)]
pub struct OrderRow {
    pub project_id: u32,
    pub order_id: String,
    pub source: String,
    pub status: String,
    #[serde(with = "clickhouse::serde::uuid::option")]
    pub visitor_id: Option<Uuid>,
    pub member_hash: String,
    pub currency: String,
    pub gross_amount: i64,
    pub net_amount: i64,
    pub refunded_amount: i64,
    pub effective_amount: i64,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    pub ordered_at: OffsetDateTime,
    pub version: u64,
}

impl From<&OrderEvent> for OrderEventRow {
    fn from(e: &OrderEvent) -> Self {
        Self {
            project_id: e.project_id,
            order_id: e.order_id.clone(),
            event_type: e.event_type.as_str().to_owned(),
            source: e.source.as_str().to_owned(),
            idempotency_key: e.idempotency_key.clone(),
            visitor_id: e.visitor_id,
            member_hash: e.member_hash.clone(),
            currency: e.currency.clone(),
            gross_amount: e.gross_amount,
            net_amount: e.net_amount,
            refund_amount: e.refund_amount,
            occurred_at: ms_to_dt(e.occurred_at_ms),
            received_at: ms_to_dt(e.received_at_ms),
            raw: e.raw.clone(),
        }
    }
}

impl OrderEventRow {
    /// 알 수 없는 event_type/source 값이면 `None`.
    pub fn to_event(&self) -> Option<OrderEvent> {
        Some(OrderEvent {
            project_id: self.project_id,
            order_id: self.order_id.clone(),
            event_type: OrderEventType::parse(&self.event_type)?,
            source: OrderSource::parse(&self.source)?,
            idempotency_key: self.idempotency_key.clone(),
            visitor_id: self.visitor_id,
            member_hash: self.member_hash.clone(),
            currency: self.currency.clone(),
            gross_amount: self.gross_amount,
            net_amount: self.net_amount,
            refund_amount: self.refund_amount,
            occurred_at_ms: dt_to_ms(self.occurred_at),
            received_at_ms: dt_to_ms(self.received_at),
            raw: self.raw.clone(),
        })
    }
}

impl OrderRow {
    pub fn from_state(s: &OrderState, version: u64) -> Self {
        Self {
            project_id: s.project_id,
            order_id: s.order_id.clone(),
            source: s.source.as_str().to_owned(),
            status: s.status.as_str().to_owned(),
            visitor_id: s.visitor_id,
            member_hash: s.member_hash.clone(),
            currency: s.currency.clone(),
            gross_amount: s.gross_amount,
            net_amount: s.net_amount,
            refunded_amount: s.refunded_amount,
            effective_amount: s.effective_amount,
            ordered_at: ms_to_dt(s.ordered_at_ms),
            version,
        }
    }

    pub fn to_state(&self) -> Option<OrderState> {
        Some(OrderState {
            project_id: self.project_id,
            order_id: self.order_id.clone(),
            source: OrderSource::parse(&self.source)?,
            status: OrderStatus::parse(&self.status)?,
            visitor_id: self.visitor_id,
            member_hash: self.member_hash.clone(),
            currency: self.currency.clone(),
            gross_amount: self.gross_amount,
            net_amount: self.net_amount,
            refunded_amount: self.refunded_amount,
            effective_amount: self.effective_amount,
            ordered_at_ms: dt_to_ms(self.ordered_at),
        })
    }
}

/// 다음 `version`: `max(prev + 1, now_ms)`. 시계가 뒤로 가도 단조 증가한다.
pub fn next_version(prev: Option<u64>, now_ms: i64) -> u64 {
    let floor = prev.map_or(1, |p| p.saturating_add(1));
    floor.max(u64::try_from(now_ms).unwrap_or(0))
}

pub async fn insert_order_event(client: &clickhouse::Client, e: &OrderEvent) -> anyhow::Result<()> {
    let mut insert = client.insert::<OrderEventRow>("order_events").await?;
    insert.write(&OrderEventRow::from(e)).await?;
    insert.end().await?;
    Ok(())
}

/// 한 주문의 모든 이벤트 (중복 행 포함, 정렬 안 됨).
/// `FINAL`을 쓰지 않는다: 엔진은 같은 키 중 늦게 받은 행을 남기지만,
/// 투영은 먼저 받은 행을 기준으로 하므로 중복 제거는 `fold`에 맡긴다.
pub async fn order_events(
    client: &clickhouse::Client,
    scope: ProjectScope,
    order_id: &str,
) -> anyhow::Result<Vec<OrderEvent>> {
    let rows: Vec<OrderEventRow> = client
        .query("SELECT ?fields FROM order_events WHERE project_id = ? AND order_id = ?")
        .bind(scope.project_id())
        .bind(order_id)
        .fetch_all()
        .await?;
    Ok(rows.iter().filter_map(OrderEventRow::to_event).collect())
}

/// 주문 현재 상태 (`FINAL`).
pub async fn current_order(
    client: &clickhouse::Client,
    scope: ProjectScope,
    order_id: &str,
) -> anyhow::Result<Option<OrderRow>> {
    Ok(client
        .query("SELECT ?fields FROM orders FINAL WHERE project_id = ? AND order_id = ?")
        .bind(scope.project_id())
        .bind(order_id)
        .fetch_optional()
        .await?)
}

pub async fn insert_order(client: &clickhouse::Client, row: &OrderRow) -> anyhow::Result<()> {
    let mut insert = client.insert::<OrderRow>("orders").await?;
    insert.write(row).await?;
    insert.end().await?;
    Ok(())
}

/// `version > watermark`인 주문 (`FINAL`, version 오름차순).
pub async fn orders_changed_since(
    client: &clickhouse::Client,
    scope: ProjectScope,
    watermark: u64,
) -> anyhow::Result<Vec<OrderRow>> {
    Ok(client
        .query(
            "SELECT ?fields FROM orders FINAL WHERE project_id = ? AND version > ? \
             ORDER BY version, order_id",
        )
        .bind(scope.project_id())
        .bind(watermark)
        .fetch_all()
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_strictly_increasing() {
        assert_eq!(next_version(None, 1_000), 1_000);
        assert_eq!(next_version(Some(500), 1_000), 1_000);
        assert_eq!(next_version(Some(1_000), 1_000), 1_001, "같은 ms");
        assert_eq!(next_version(Some(5_000), 1_000), 5_001, "시계 역행");
        assert_eq!(next_version(None, -5), 1, "음수 시각");
    }
}

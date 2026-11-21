//! 주문 이벤트(`order_events`)와 주문 현재 상태(`orders`) 읽기·쓰기.
//!
//! 다른 패키지와의 계약:
//! - `orders`는 항상 `FINAL`로 읽는다 (ReplacingMergeTree(version)).
//! - `version`은 unix ms 근사값이며 주문마다 엄격히 증가한다: `max(이전 version + 1, now_ms)`.
//! - 기여 연산 잡은 `version > watermark`로 바뀐 주문을 찾는다 ([`orders_changed_since`]).
//! - `order_events`는 append-only다. 같은 멱등 키의 중복 행이 있을 수 있으므로
//!   읽는 쪽은 [`analytics_core::orders::fold`]처럼 멱등 키로 중복을 제거한다.

use crate::ProjectScope;
use analytics_core::orders::{OrderEvent, OrderEventType, OrderSource, OrderState, OrderStatus};
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

fn ms_to_dt(ms: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn dt_to_ms(t: OffsetDateTime) -> i64 {
    (t.unix_timestamp_nanos() / 1_000_000) as i64
}

impl From<&OrderEvent> for OrderEventRow {
    fn from(e: &OrderEvent) -> Self {
        let _ = (e, ms_to_dt(0));
        todo!()
    }
}

impl OrderEventRow {
    /// 알 수 없는 event_type/source 값이면 `None`.
    pub fn to_event(&self) -> Option<OrderEvent> {
        let _ = (OrderEventType::parse(""), OrderSource::parse(""), dt_to_ms);
        todo!()
    }
}

impl OrderRow {
    pub fn from_state(s: &OrderState, version: u64) -> Self {
        let _ = (s, version);
        todo!()
    }

    pub fn to_state(&self) -> Option<OrderState> {
        let _ = OrderStatus::parse("");
        todo!()
    }
}

/// 다음 `version`: `max(prev + 1, now_ms)`. 시계가 뒤로 가도 단조 증가한다.
pub fn next_version(prev: Option<u64>, now_ms: i64) -> u64 {
    let _ = (prev, now_ms);
    todo!()
}

pub async fn insert_order_event(client: &clickhouse::Client, e: &OrderEvent) -> anyhow::Result<()> {
    let _ = (client, e);
    todo!()
}

/// 한 주문의 모든 이벤트 (중복 행 포함, 정렬 안 됨).
pub async fn order_events(
    client: &clickhouse::Client,
    scope: ProjectScope,
    order_id: &str,
) -> anyhow::Result<Vec<OrderEvent>> {
    let _ = (client, scope, order_id);
    todo!()
}

/// 주문 현재 상태 (`FINAL`).
pub async fn current_order(
    client: &clickhouse::Client,
    scope: ProjectScope,
    order_id: &str,
) -> anyhow::Result<Option<OrderRow>> {
    let _ = (client, scope, order_id);
    todo!()
}

pub async fn insert_order(client: &clickhouse::Client, row: &OrderRow) -> anyhow::Result<()> {
    let _ = (client, row);
    todo!()
}

/// `version > watermark`인 주문 (`FINAL`, version 오름차순).
pub async fn orders_changed_since(
    client: &clickhouse::Client,
    scope: ProjectScope,
    watermark: u64,
) -> anyhow::Result<Vec<OrderRow>> {
    let _ = (client, scope, watermark);
    todo!()
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

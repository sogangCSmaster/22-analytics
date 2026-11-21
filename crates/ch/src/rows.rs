//! ClickHouse 행 타입.

use analytics_core::Event;
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Row, Serialize, Deserialize)]
pub struct EventRow {
    pub project_id: u32,
    #[serde(with = "clickhouse::serde::uuid")]
    pub event_id: Uuid,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    pub ts: OffsetDateTime,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    pub received_at: OffsetDateTime,
    pub event_name: String,
    #[serde(with = "clickhouse::serde::uuid")]
    pub visitor_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    pub session_id: Uuid,
    pub is_session_start: u8,
    pub member_hash: String,
    pub url: String,
    pub host: String,
    pub path: String,
    pub title: String,
    pub referrer: String,
    pub referrer_host: String,
    pub utm_source: String,
    pub utm_medium: String,
    pub utm_campaign: String,
    pub utm_content: String,
    pub utm_term: String,
    pub click_id: String,
    pub click_id_type: String,
    pub channel: String,
    pub country: String,
    pub region: String,
    pub city: String,
    pub device_type: String,
    pub browser: String,
    pub os: String,
    pub screen_w: u16,
    #[serde(rename = "props.key")]
    pub props_key: Vec<String>,
    #[serde(rename = "props.value")]
    pub props_value: Vec<String>,
    pub ec_order_id: String,
    pub ec_value: i64,
    pub ec_currency: String,
    #[serde(rename = "items.id")]
    pub items_id: Vec<String>,
    #[serde(rename = "items.name")]
    pub items_name: Vec<String>,
    #[serde(rename = "items.price")]
    pub items_price: Vec<i64>,
    #[serde(rename = "items.qty")]
    pub items_qty: Vec<u32>,
}

fn ms_to_dt(ms: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

impl From<&Event> for EventRow {
    fn from(e: &Event) -> Self {
        let (props_key, props_value) = e.props.iter().cloned().unzip();
        let ec = e.ec.clone().unwrap_or_default();
        Self {
            project_id: e.project_id,
            event_id: e.event_id,
            ts: ms_to_dt(e.ts_ms),
            received_at: ms_to_dt(e.received_at_ms),
            event_name: e.event_name.clone(),
            visitor_id: e.visitor_id,
            session_id: e.session_id,
            is_session_start: e.is_session_start as u8,
            member_hash: e.member_hash.clone(),
            url: e.url.clone(),
            host: e.host.clone(),
            path: e.path.clone(),
            title: e.title.clone(),
            referrer: e.referrer.clone(),
            referrer_host: e.referrer_host.clone(),
            utm_source: e.utm_source.clone(),
            utm_medium: e.utm_medium.clone(),
            utm_campaign: e.utm_campaign.clone(),
            utm_content: e.utm_content.clone(),
            utm_term: e.utm_term.clone(),
            click_id: e.click_id.clone(),
            click_id_type: e.click_id_type.clone(),
            channel: e.channel.clone(),
            country: e.country.clone(),
            region: e.region.clone(),
            city: e.city.clone(),
            device_type: e.device_type.clone(),
            browser: e.browser.clone(),
            os: e.os.clone(),
            screen_w: e.screen_w,
            props_key,
            props_value,
            ec_order_id: ec.order_id,
            ec_value: ec.value,
            ec_currency: ec.currency,
            items_id: ec.items.iter().map(|i| i.id.clone()).collect(),
            items_name: ec.items.iter().map(|i| i.name.clone()).collect(),
            items_price: ec.items.iter().map(|i| i.price).collect(),
            items_qty: ec.items.iter().map(|i| i.qty).collect(),
        }
    }
}

/// 이벤트 배치를 한 번의 INSERT로 쓴다.
pub async fn insert_events(client: &clickhouse::Client, events: &[Event]) -> anyhow::Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let mut insert = client.insert::<EventRow>("events").await?;
    for e in events {
        insert.write(&EventRow::from(e)).await?;
    }
    insert.end().await?;
    Ok(())
}

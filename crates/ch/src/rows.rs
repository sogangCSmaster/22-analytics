//! ClickHouse 행 타입.

use analytics_core::Event;
use analytics_core::timeutil::ms_to_dt;
use clickhouse::Row;
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

/// `events` INSERT용 행. 이벤트의 문자열을 빌려 쓰므로 행마다 복사하지 않는다.
/// (`events`를 행 단위로 다시 읽는 코드는 없어서 소유 타입은 두지 않는다.)
#[derive(Debug, Clone, Row, Serialize)]
pub struct EventRow<'a> {
    pub project_id: u32,
    #[serde(with = "clickhouse::serde::uuid")]
    pub event_id: Uuid,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    pub ts: OffsetDateTime,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    pub received_at: OffsetDateTime,
    pub event_name: &'a str,
    #[serde(with = "clickhouse::serde::uuid")]
    pub visitor_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    pub session_id: Uuid,
    pub is_session_start: u8,
    pub member_hash: &'a str,
    pub url: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    pub title: &'a str,
    pub referrer: &'a str,
    pub referrer_host: &'a str,
    pub utm_source: &'a str,
    pub utm_medium: &'a str,
    pub utm_campaign: &'a str,
    pub utm_content: &'a str,
    pub utm_term: &'a str,
    pub click_id: &'a str,
    pub click_id_type: &'a str,
    pub channel: &'a str,
    pub country: &'a str,
    pub region: &'a str,
    pub city: &'a str,
    pub device_type: &'a str,
    pub browser: &'a str,
    pub os: &'a str,
    pub screen_w: u16,
    #[serde(rename = "props.key")]
    pub props_key: Vec<&'a str>,
    #[serde(rename = "props.value")]
    pub props_value: Vec<&'a str>,
    pub ec_order_id: &'a str,
    pub ec_value: i64,
    pub ec_currency: &'a str,
    #[serde(rename = "items.id")]
    pub items_id: Vec<&'a str>,
    #[serde(rename = "items.name")]
    pub items_name: Vec<&'a str>,
    #[serde(rename = "items.price")]
    pub items_price: Vec<i64>,
    #[serde(rename = "items.qty")]
    pub items_qty: Vec<u32>,
}

impl<'a> From<&'a Event> for EventRow<'a> {
    fn from(e: &'a Event) -> Self {
        let (props_key, props_value) = e
            .props
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .unzip();
        let ec = e.ec.as_ref();
        let items = ec.map(|ec| ec.items.as_slice()).unwrap_or_default();
        Self {
            project_id: e.project_id,
            event_id: e.event_id,
            ts: ms_to_dt(e.ts_ms),
            received_at: ms_to_dt(e.received_at_ms),
            event_name: &e.event_name,
            visitor_id: e.visitor_id,
            session_id: e.session_id,
            is_session_start: e.is_session_start as u8,
            member_hash: &e.member_hash,
            url: &e.url,
            host: &e.host,
            path: &e.path,
            title: &e.title,
            referrer: &e.referrer,
            referrer_host: &e.referrer_host,
            utm_source: &e.utm_source,
            utm_medium: &e.utm_medium,
            utm_campaign: &e.utm_campaign,
            utm_content: &e.utm_content,
            utm_term: &e.utm_term,
            click_id: &e.click_id,
            click_id_type: &e.click_id_type,
            channel: &e.channel,
            country: &e.country,
            region: &e.region,
            city: &e.city,
            device_type: &e.device_type,
            browser: &e.browser,
            os: &e.os,
            screen_w: e.screen_w,
            props_key,
            props_value,
            ec_order_id: ec.map_or("", |ec| ec.order_id.as_str()),
            ec_value: ec.map_or(0, |ec| ec.value),
            ec_currency: ec.map_or("", |ec| ec.currency.as_str()),
            items_id: items.iter().map(|i| i.id.as_str()).collect(),
            items_name: items.iter().map(|i| i.name.as_str()).collect(),
            items_price: items.iter().map(|i| i.price).collect(),
            items_qty: items.iter().map(|i| i.qty).collect(),
        }
    }
}

/// 이벤트 배치를 한 번의 INSERT로 쓴다.
pub async fn insert_events(client: &clickhouse::Client, events: &[Event]) -> anyhow::Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let mut insert = client.insert::<EventRow<'_>>("events").await?;
    for e in events {
        insert.write(&EventRow::from(e)).await?;
    }
    insert.end().await?;
    Ok(())
}

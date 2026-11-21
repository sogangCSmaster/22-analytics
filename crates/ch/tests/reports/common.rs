//! 리포트 통합 테스트 공용 픽스처.

use analytics_ch::rows::insert_events;
use analytics_ch::{Client, ProjectScope};
use analytics_core::Event;
use clickhouse::Row;
use serde::Serialize;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset, macros::format_description};
use uuid::Uuid;

/// 기본 테스트 프로젝트
pub const P: u32 = 101;
/// 다른 테넌트
pub const Q: u32 = 202;

pub fn scope(p: u32) -> ProjectScope {
    ProjectScope::authorized(p)
}

/// `"2026-11-20 18:45:00"` (Asia/Seoul) → OffsetDateTime
pub fn kst(s: &str) -> OffsetDateTime {
    let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    PrimitiveDateTime::parse(s, fmt)
        .expect("시각 형식")
        .assume_offset(UtcOffset::from_hms(9, 0, 0).unwrap())
}

pub fn ms(t: OffsetDateTime) -> i64 {
    (t.unix_timestamp_nanos() / 1_000_000) as i64
}

pub fn uid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// 이벤트 빌더. 기본값: page_view, path "/", channel "direct", 세션 시작 아님.
pub struct Ev(Event);

pub fn ev(project: u32, name: &str, visitor: u128, session: u128, at: &str) -> Ev {
    ev_at(project, name, visitor, session, kst(at))
}

pub fn ev_at(project: u32, name: &str, visitor: u128, session: u128, at: OffsetDateTime) -> Ev {
    let ts = ms(at);
    Ev(Event {
        project_id: project,
        event_id: Uuid::new_v4(),
        ts_ms: ts,
        received_at_ms: ts,
        event_name: name.into(),
        visitor_id: uid(visitor),
        session_id: uid(session),
        is_session_start: false,
        member_hash: String::new(),
        url: "https://shop.test/".into(),
        host: "shop.test".into(),
        path: "/".into(),
        title: String::new(),
        referrer: String::new(),
        referrer_host: String::new(),
        utm_source: String::new(),
        utm_medium: String::new(),
        utm_campaign: String::new(),
        utm_content: String::new(),
        utm_term: String::new(),
        click_id: String::new(),
        click_id_type: String::new(),
        channel: "direct".into(),
        country: "KR".into(),
        region: String::new(),
        city: String::new(),
        device_type: "desktop".into(),
        browser: String::new(),
        os: String::new(),
        screen_w: 1440,
        props: vec![],
        ec: None,
    })
}

impl Ev {
    /// 세션 시작 이벤트로 표시하고 세션 채널을 지정한다.
    pub fn start(mut self, channel: &str) -> Self {
        self.0.is_session_start = true;
        self.0.channel = channel.into();
        self
    }
    /// 세션 채널 (세션의 모든 이벤트가 같은 값을 갖는다).
    pub fn ch(mut self, channel: &str) -> Self {
        self.0.channel = channel.into();
        self
    }
    pub fn path(mut self, path: &str) -> Self {
        self.0.path = path.into();
        self.0.url = format!("https://shop.test{path}");
        self
    }
    pub fn host(mut self, host: &str) -> Self {
        self.0.host = host.into();
        self.0.url = format!("https://{host}{}", self.0.path);
        self
    }
    pub fn prop(mut self, k: &str, v: &str) -> Self {
        self.0.props.push((k.into(), v.into()));
        self
    }
    pub fn build(self) -> Event {
        self.0
    }
}

pub async fn insert(ch: &Client, evs: Vec<Ev>) {
    let evs: Vec<Event> = evs.into_iter().map(Ev::build).collect();
    insert_events(ch, &evs).await.expect("이벤트 적재");
}

#[derive(Row, Serialize)]
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

/// 주문 한 건. status: CONFIRMED / PARTIALLY_REFUNDED / CANCELLED
pub fn order(
    project: u32,
    id: &str,
    status: &str,
    net: i64,
    refunded: i64,
    at: &str,
    version: u64,
) -> OrderRow {
    let effective = if status == "CANCELLED" {
        0
    } else {
        (net - refunded).max(0)
    };
    OrderRow {
        project_id: project,
        order_id: id.into(),
        source: "s2s".into(),
        status: status.into(),
        visitor_id: Some(uid(1)),
        member_hash: String::new(),
        currency: "KRW".into(),
        gross_amount: net + 10_000,
        net_amount: net,
        refunded_amount: refunded,
        effective_amount: effective,
        ordered_at: kst(at),
        version,
    }
}

pub async fn insert_orders(ch: &Client, rows: Vec<OrderRow>) {
    let mut ins = ch.insert::<OrderRow>("orders").await.unwrap();
    for r in &rows {
        ins.write(r).await.unwrap();
    }
    ins.end().await.unwrap();
}

#[derive(Row, Serialize, Clone)]
pub struct AttrRow {
    pub project_id: u32,
    pub model: String,
    pub lookback_days: u8,
    pub order_id: String,
    pub touch_index: u16,
    pub channel: String,
    pub utm_campaign: String,
    #[serde(with = "clickhouse::serde::time::datetime64::millis::option")]
    pub touch_ts: Option<OffsetDateTime>,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    pub ordered_at: OffsetDateTime,
    pub amount: i64,
    pub is_assist: u8,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    pub computed_at: OffsetDateTime,
    pub is_deleted: u8,
}

/// 기여 연산 결과 묶음 빌더 (WP2 잡이 쓰는 형태를 흉내 낸다).
pub struct Attr {
    pub project: u32,
    pub order_id: String,
    pub ordered_at: OffsetDateTime,
    pub lookback: u8,
    pub computed_at: OffsetDateTime,
    pub deleted: bool,
}

pub fn attr(project: u32, order_id: &str, ordered_at: &str, lookback: u8) -> Attr {
    Attr {
        project,
        order_id: order_id.into(),
        ordered_at: kst(ordered_at),
        lookback,
        computed_at: kst("2026-11-22 04:00:00"),
        deleted: false,
    }
}

impl Attr {
    pub fn computed(mut self, at: &str) -> Self {
        self.computed_at = kst(at);
        self
    }
    pub fn deleted(mut self) -> Self {
        self.deleted = true;
        self
    }
    /// 한 모델의 배분 행. `rows`: (touch_index, channel, 접점 시각, 금액, 어시스트)
    pub fn model(
        &self,
        model: &str,
        rows: &[(u16, &str, Option<&str>, i64, bool)],
    ) -> Vec<AttrRow> {
        rows.iter()
            .map(|(i, ch, ts, amount, assist)| AttrRow {
                project_id: self.project,
                model: model.into(),
                lookback_days: self.lookback,
                order_id: self.order_id.clone(),
                touch_index: *i,
                channel: (*ch).into(),
                utm_campaign: format!("cmp_{ch}"),
                touch_ts: ts.map(kst),
                ordered_at: self.ordered_at,
                amount: *amount,
                is_assist: *assist as u8,
                computed_at: self.computed_at,
                is_deleted: self.deleted as u8,
            })
            .collect()
    }

    /// 접점이 하나(또는 미귀속)인 주문: 모든 모델이 같은 행.
    pub fn single(&self, index: u16, channel: &str, ts: Option<&str>, amount: i64) -> Vec<AttrRow> {
        [
            "last_click",
            "first_click",
            "linear",
            "time_decay",
            "raw_sum",
        ]
        .iter()
        .flat_map(|m| self.model(m, &[(index, channel, ts, amount, false)]))
        .collect()
    }
}

pub async fn insert_attr(ch: &Client, rows: Vec<AttrRow>) {
    let mut ins = ch.insert::<AttrRow>("attributed_revenue").await.unwrap();
    for r in &rows {
        ins.write(r).await.unwrap();
    }
    ins.end().await.unwrap();
}

/// 인젝션 시도 문자열.
pub const NASTY: &[&str] = &[
    "it's",
    "back\\slash",
    "50%_off",
    "'; DROP TABLE events; --",
    "\"quoted\"",
    "?",
    "{p:String}",
];

//! S2S 주문 API: 전송 포맷, 검증, 주문 상태 투영 (ARCHITECTURE §5).
//!
//! - [`validate`]: 고객사 서버가 보낸 주문 이벤트 1건을 검증·정규화해 [`OrderEvent`]를 만든다.
//! - [`fold`]: 한 주문의 모든 이벤트로 현재 상태([`OrderState`])를 계산한다.
//!   입력 순서와 무관하고(정렬 후 계산), 같은 멱등 키는 한 번만 반영한다.
//! - [`client_purchase_event`]: 트래커 `purchase` 이벤트를 `source='client'` 주문 이벤트로 바꾼다.
//! - [`project`]: `orders`에 쓸 상태. [`fold`]에 더해 이전 행의 `ordered_at`을 유지한다 (파티션 고정).
//!
//! I/O는 하지 않는다. 현재 시각은 호출자가 주입한다.

use crate::collect::rehash_member;
use crate::event::Event;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// `order_id` 최대 길이 (문자 수).
pub const MAX_ORDER_ID_CHARS: usize = 128;
/// 금액 상한 (통화 최소단위). 1조.
pub const MAX_AMOUNT: i64 = 1_000_000_000_000;
/// `POST /v1/orders` 본문 상한.
pub const MAX_SINGLE_BODY_BYTES: usize = 64 * 1024;
/// `POST /v1/orders:batch` 본문 상한.
pub const MAX_BATCH_BODY_BYTES: usize = 1024 * 1024;
/// 일괄 전송 최대 건수.
pub const MAX_BATCH_ORDERS: usize = 500;
/// 멱등 키 최대 길이.
pub const MAX_IDEMPOTENCY_KEY_LEN: usize = 255;
/// 미래 시각 허용 한도 (시계 오차).
pub const MAX_FUTURE_MS: i64 = 24 * 60 * 60 * 1000;
/// 2000-01-01T00:00:00Z (epoch ms). 이보다 이른 시각은 거부한다.
pub const MIN_OCCURRED_AT_MS: i64 = 946_684_800_000;

/// 경고 코드: `visitor_id`가 UUID가 아니어서 없는 것으로 처리했다.
pub const WARN_INVALID_VISITOR_ID: &str = "invalid_visitor_id";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum OrderEventType {
    #[serde(rename = "PURCHASE_CONFIRMED")]
    PurchaseConfirmed,
    #[serde(rename = "ORDER_CANCELLED")]
    OrderCancelled,
    #[serde(rename = "ORDER_REFUNDED")]
    OrderRefunded,
}

impl OrderEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PurchaseConfirmed => "PURCHASE_CONFIRMED",
            Self::OrderCancelled => "ORDER_CANCELLED",
            Self::OrderRefunded => "ORDER_REFUNDED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "PURCHASE_CONFIRMED" => Some(Self::PurchaseConfirmed),
            "ORDER_CANCELLED" => Some(Self::OrderCancelled),
            "ORDER_REFUNDED" => Some(Self::OrderRefunded),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderSource {
    S2s,
    Client,
}

impl OrderSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::S2s => "s2s",
            Self::Client => "client",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "s2s" => Some(Self::S2s),
            "client" => Some(Self::Client),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderStatus {
    /// 환불·취소 이벤트만 있고 확정이 아직 없다.
    Pending,
    Confirmed,
    PartiallyRefunded,
    Cancelled,
}

impl OrderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Confirmed => "CONFIRMED",
            Self::PartiallyRefunded => "PARTIALLY_REFUNDED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "PENDING" => Some(Self::Pending),
            "CONFIRMED" => Some(Self::Confirmed),
            "PARTIALLY_REFUNDED" => Some(Self::PartiallyRefunded),
            "CANCELLED" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// S2S 전송 포맷 1건 (ARCHITECTURE §5.1). 세 이벤트 유형의 필드를 모두 담는다.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WireOrder {
    pub event_type: String,
    pub order_id: String,
    #[serde(default)]
    pub visitor_id: Option<String>,
    #[serde(default)]
    pub member_id_hash: Option<String>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub gross_amount: Option<i64>,
    #[serde(default)]
    pub net_amount: Option<i64>,
    #[serde(default)]
    pub refund_amount: Option<i64>,
    #[serde(default)]
    pub ordered_at: Option<String>,
    #[serde(default)]
    pub cancelled_at: Option<String>,
    #[serde(default)]
    pub refunded_at: Option<String>,
    /// 일괄 전송에서 항목별 멱등 키 (단건은 `Idempotency-Key` 헤더).
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// 검증된 주문 이벤트. ClickHouse `order_events` 행 1개와 대응한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderEvent {
    pub project_id: u32,
    pub order_id: String,
    pub event_type: OrderEventType,
    pub source: OrderSource,
    pub idempotency_key: String,
    pub visitor_id: Option<Uuid>,
    /// 프로젝트 salt로 재해싱한 값. 없으면 ''.
    pub member_hash: String,
    pub currency: String,
    pub gross_amount: i64,
    pub net_amount: i64,
    pub refund_amount: i64,
    pub occurred_at_ms: i64,
    pub received_at_ms: i64,
    /// 원본 JSON (감사용)
    pub raw: String,
}

impl OrderEvent {
    /// 같은 멱등 키의 두 이벤트가 같은 요청(재전송)인지. 수신 시각·원문·식별자는 비교하지 않고
    /// 금액과 발생 시각만 본다 (기본 멱등 키는 이 값들로 만들므로 기본 키끼리는 항상 같다).
    pub fn same_payload(&self, other: &OrderEvent) -> bool {
        (
            self.event_type,
            self.net_amount,
            self.refund_amount,
            self.occurred_at_ms,
        ) == (
            other.event_type,
            other.net_amount,
            other.refund_amount,
            other.occurred_at_ms,
        )
    }
}

/// 주문 현재 상태. ClickHouse `orders` 행과 대응한다 (`version` 제외).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderState {
    pub project_id: u32,
    pub order_id: String,
    pub source: OrderSource,
    pub status: OrderStatus,
    pub visitor_id: Option<Uuid>,
    pub member_hash: String,
    pub currency: String,
    pub gross_amount: i64,
    pub net_amount: i64,
    pub refunded_amount: i64,
    /// max(net_amount − Σ환불, 0). 취소면 0.
    pub effective_amount: i64,
    pub ordered_at_ms: i64,
}

/// 검증에 필요한 프로젝트 정보.
#[derive(Debug, Clone)]
pub struct OrderProject<'a> {
    pub project_id: u32,
    /// 프로젝트 통화 (기본 KRW)
    pub currency: &'a str,
    /// 회원 해시 재해싱용 salt (hex)
    pub salt_hex: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Validated {
    pub event: OrderEvent,
    pub warnings: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OrderError {
    #[error("요청 형식이 올바르지 않습니다: {0}")]
    InvalidPayload(String),
    #[error("알 수 없는 event_type")]
    UnknownEventType,
    #[error("order_id는 1~128자이고 제어 문자를 포함할 수 없습니다")]
    InvalidOrderId,
    #[error("필수 필드 누락: {0}")]
    MissingField(&'static str),
    #[error("금액 범위 초과 (0 ~ 1조): {0}")]
    AmountOutOfRange(&'static str),
    #[error("프로젝트 통화와 다른 통화")]
    CurrencyMismatch,
    #[error("RFC3339 시각이 아닙니다: {0}")]
    InvalidTimestamp(&'static str),
    #[error("허용 범위를 벗어난 시각 (2000-01-01 ~ 현재+1일): {0}")]
    TimestampOutOfRange(&'static str),
    #[error("member_id_hash는 sha256 hex(64자)여야 합니다")]
    InvalidMemberHash,
    #[error("멱등 키는 1~255자의 출력 가능한 ASCII여야 합니다")]
    InvalidIdempotencyKey,
}

impl OrderError {
    /// API 응답 `error` 코드.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidPayload(_) => "invalid_payload",
            Self::UnknownEventType => "unknown_event_type",
            Self::InvalidOrderId => "invalid_order_id",
            Self::MissingField(_) => "missing_field",
            Self::AmountOutOfRange(_) => "amount_out_of_range",
            Self::CurrencyMismatch => "currency_mismatch",
            Self::InvalidTimestamp(_) => "invalid_timestamp",
            Self::TimestampOutOfRange(_) => "timestamp_out_of_range",
            Self::InvalidMemberHash => "invalid_member_hash",
            Self::InvalidIdempotencyKey => "invalid_idempotency_key",
        }
    }
}

/// 기본 멱등 키: `sha256(order_id|event_type|amount|occurred_at_ms)` hex.
/// amount는 확정이면 net_amount, 취소·환불이면 refund_amount.
pub fn default_idempotency_key(
    order_id: &str,
    event_type: OrderEventType,
    amount: i64,
    occurred_at_ms: i64,
) -> String {
    let mut h = Sha256::new();
    h.update(order_id.as_bytes());
    h.update(b"|");
    h.update(event_type.as_str().as_bytes());
    h.update(b"|");
    h.update(amount.to_string().as_bytes());
    h.update(b"|");
    h.update(occurred_at_ms.to_string().as_bytes());
    hex::encode(h.finalize())
}

/// S2S 주문 이벤트 1건을 검증한다. `raw`는 원본 JSON 값이다.
pub fn validate(
    project: &OrderProject<'_>,
    raw: &serde_json::Value,
    idempotency_key: Option<&str>,
    now_ms: i64,
) -> Result<Validated, OrderError> {
    let obj = raw
        .as_object()
        .ok_or_else(|| OrderError::InvalidPayload("JSON 객체가 아닙니다".into()))?;
    // visitor_id는 형식이 틀려도 주문을 거부하지 않는다 (경고만). 타입과 무관하게 따로 읽는다.
    let mut warnings = Vec::new();
    let visitor_id = match obj.get("visitor_id") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) if s.is_empty() => None,
        Some(v) => {
            let parsed = v
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .filter(|u| !u.is_nil());
            if parsed.is_none() {
                warnings.push(WARN_INVALID_VISITOR_ID);
            }
            parsed
        }
    };
    let mut fields = obj.clone();
    fields.remove("visitor_id");
    let w: WireOrder = serde_json::from_value(serde_json::Value::Object(fields))
        .map_err(|e| OrderError::InvalidPayload(e.to_string()))?;

    let event_type = OrderEventType::parse(&w.event_type).ok_or(OrderError::UnknownEventType)?;
    if !valid_order_id(&w.order_id) {
        return Err(OrderError::InvalidOrderId);
    }

    let currency = match w.currency.as_deref() {
        None | Some("") => project.currency.to_ascii_uppercase(),
        Some(c) if c.eq_ignore_ascii_case(project.currency) => c.to_ascii_uppercase(),
        Some(_) => return Err(OrderError::CurrencyMismatch),
    };

    let member_hash = match w.member_id_hash.as_deref() {
        None | Some("") => String::new(),
        Some(m) if is_sha256_hex(m) => rehash_member(project.salt_hex, m),
        // 이메일·전화번호 같은 평문으로 보이는 값을 포함해 모두 거부한다 (§9).
        Some(_) => return Err(OrderError::InvalidMemberHash),
    };

    let (gross_amount, net_amount, refund_amount, at_field, at) = match event_type {
        OrderEventType::PurchaseConfirmed => {
            let net = amount(w.net_amount, "net_amount")?
                .ok_or(OrderError::MissingField("net_amount"))?;
            let gross = amount(w.gross_amount, "gross_amount")?.unwrap_or(net);
            (gross, net, 0, "ordered_at", w.ordered_at)
        }
        OrderEventType::OrderCancelled => {
            let refund = amount(w.refund_amount, "refund_amount")?.unwrap_or(0);
            (0, 0, refund, "cancelled_at", w.cancelled_at)
        }
        OrderEventType::OrderRefunded => {
            let refund = amount(w.refund_amount, "refund_amount")?
                .ok_or(OrderError::MissingField("refund_amount"))?;
            (0, 0, refund, "refunded_at", w.refunded_at)
        }
    };
    let at = at.ok_or(OrderError::MissingField(at_field))?;
    let occurred_at_ms = parse_rfc3339_ms(&at).ok_or(OrderError::InvalidTimestamp(at_field))?;
    if !(MIN_OCCURRED_AT_MS..=now_ms.saturating_add(MAX_FUTURE_MS)).contains(&occurred_at_ms) {
        return Err(OrderError::TimestampOutOfRange(at_field));
    }

    let idempotency_key = match idempotency_key.or(w.idempotency_key.as_deref()) {
        // 명시 키는 이벤트 유형으로 네임스페이스를 붙인다: 가맹점이 확정·환불에 같은 키
        // (예: order_id)를 보내도 서로를 중복으로 판정하지 않는다. 기본 키는 이미 유형을 포함한다.
        Some(k) if valid_idempotency_key(k) => format!("{}:{k}", event_type.as_str()),
        Some(_) => return Err(OrderError::InvalidIdempotencyKey),
        None => {
            let amt = match event_type {
                OrderEventType::PurchaseConfirmed => net_amount,
                _ => refund_amount,
            };
            default_idempotency_key(&w.order_id, event_type, amt, occurred_at_ms)
        }
    };

    Ok(Validated {
        event: OrderEvent {
            project_id: project.project_id,
            order_id: w.order_id,
            event_type,
            source: OrderSource::S2s,
            idempotency_key,
            visitor_id,
            member_hash,
            currency,
            gross_amount,
            net_amount,
            refund_amount,
            occurred_at_ms,
            received_at_ms: now_ms,
            raw: raw.to_string(),
        },
        warnings,
    })
}

/// 트래커 `purchase` 이벤트 → `source='client'` 확정 이벤트.
/// `order_id`가 없거나, 통화가 프로젝트와 다르거나, 금액이 범위를 벗어나면 `None`.
pub fn client_purchase_event(project_currency: &str, e: &Event) -> Option<OrderEvent> {
    if e.event_name != PURCHASE_EVENT {
        return None;
    }
    let ec = e.ec.as_ref()?;
    if !valid_order_id(&ec.order_id)
        || !(0..=MAX_AMOUNT).contains(&ec.value)
        || !ec.currency.eq_ignore_ascii_case(project_currency)
    {
        return None;
    }
    Some(OrderEvent {
        project_id: e.project_id,
        order_id: ec.order_id.clone(),
        event_type: OrderEventType::PurchaseConfirmed,
        source: OrderSource::Client,
        // 트래커 재전송은 같은 event_id를 쓰므로 한 번만 반영된다.
        idempotency_key: format!("client:{}", e.event_id),
        visitor_id: Some(e.visitor_id).filter(|v| !v.is_nil()),
        member_hash: e.member_hash.clone(),
        currency: ec.currency.to_ascii_uppercase(),
        gross_amount: ec.value,
        net_amount: ec.value,
        refund_amount: 0,
        occurred_at_ms: e.ts_ms,
        received_at_ms: e.received_at_ms,
        raw: serde_json::to_string(ec).unwrap_or_default(),
    })
}

/// 트래커 구매 이벤트 이름.
pub const PURCHASE_EVENT: &str = "purchase";

/// 한 주문의 이벤트 전부 → 현재 상태. 이벤트가 없으면 `None`.
///
/// 규칙:
/// - 같은 멱등 키는 가장 먼저 받은 것 하나만 반영한다.
/// - (occurred_at, received_at, 멱등 키) 순으로 정렬해 계산하므로 입력 순서와 무관하다.
/// - S2S 이벤트가 하나라도 있으면 클라이언트 이벤트는 무시한다 (S2S 우선).
/// - 확정이 여러 번이면 S2S는 가장 늦은 확정, 클라이언트만 있으면 가장 이른 확정의 금액을 쓴다
///   (매출은 한 번만).
/// - 취소가 있으면 도착 순서와 관계없이 CANCELLED (effective 0).
/// - 환불은 모두 합산한다. 합계가 net 이상이면 CANCELLED와 같게 취급한다.
pub fn fold(events: &[OrderEvent]) -> Option<OrderState> {
    let has_s2s = events.iter().any(|e| e.source == OrderSource::S2s);
    let mut evs: Vec<&OrderEvent> = events
        .iter()
        .filter(|e| !has_s2s || e.source == OrderSource::S2s)
        .collect();
    // 멱등 키 중복 제거: 먼저 받은 것 하나 (동률이면 내용 순으로 결정적으로).
    evs.sort_by(|a, b| {
        (
            &a.idempotency_key,
            a.received_at_ms,
            a.occurred_at_ms,
            &a.raw,
        )
            .cmp(&(
                &b.idempotency_key,
                b.received_at_ms,
                b.occurred_at_ms,
                &b.raw,
            ))
    });
    evs.dedup_by(|b, a| a.idempotency_key == b.idempotency_key);
    evs.sort_by(|a, b| {
        (a.occurred_at_ms, a.received_at_ms, &a.idempotency_key).cmp(&(
            b.occurred_at_ms,
            b.received_at_ms,
            &b.idempotency_key,
        ))
    });
    let first = *evs.first()?;

    let is_confirm = |e: &&&OrderEvent| e.event_type == OrderEventType::PurchaseConfirmed;
    // 반영할 확정 순서: S2S는 늦은 확정 우선(금액 정정), 클라이언트만 있으면 첫 확정 우선
    // (공개 키만 있으면 누구나 purchase를 보낼 수 있으므로 나중 이벤트가 금액을 바꾸지 못하게 한다).
    let confirms: Vec<&OrderEvent> = if has_s2s {
        evs.iter().rev().filter(is_confirm).copied().collect()
    } else {
        evs.iter().filter(is_confirm).copied().collect()
    };
    let chosen_confirm = confirms.first().copied();
    let cancelled = evs
        .iter()
        .any(|e| e.event_type == OrderEventType::OrderCancelled);
    let refunds = evs
        .iter()
        .filter(|e| e.event_type == OrderEventType::OrderRefunded)
        .fold(0i64, |acc, e| acc.saturating_add(e.refund_amount));

    // 식별자: 반영한 확정 순서대로, 없으면 다른 이벤트 중 가장 늦은 값.
    let mut by_priority = confirms.iter().chain(evs.iter().rev());
    let visitor_id = by_priority.clone().find_map(|e| e.visitor_id);
    let member_hash = by_priority
        .find(|e| !e.member_hash.is_empty())
        .map(|e| e.member_hash.clone())
        .unwrap_or_default();

    let (net_amount, gross_amount, ordered_at_ms, currency) = match chosen_confirm {
        Some(c) => (
            c.net_amount,
            c.gross_amount,
            c.occurred_at_ms,
            c.currency.clone(),
        ),
        None => (0, 0, first.occurred_at_ms, first.currency.clone()),
    };
    let (status, refunded_amount, effective_amount) = if cancelled {
        (OrderStatus::Cancelled, refunds.max(net_amount), 0)
    } else if chosen_confirm.is_none() {
        (OrderStatus::Pending, refunds, 0)
    } else if net_amount > 0 && refunds >= net_amount {
        (OrderStatus::Cancelled, refunds, 0)
    } else if refunds > 0 {
        (
            OrderStatus::PartiallyRefunded,
            refunds,
            net_amount - refunds,
        )
    } else {
        (OrderStatus::Confirmed, 0, net_amount)
    };

    Some(OrderState {
        project_id: first.project_id,
        order_id: first.order_id.clone(),
        source: first.source,
        status,
        visitor_id,
        member_hash,
        currency,
        gross_amount,
        net_amount,
        refunded_amount,
        effective_amount,
        ordered_at_ms,
    })
}

/// `orders`에 쓸 상태: [`fold`] + **`ordered_at` 고정**.
///
/// `orders`(와 `attributed_revenue`)는 `ReplacingMergeTree`이고 `PARTITION BY toYYYYMM(ordered_at)`이다.
/// 엔진은 파티션을 넘어 중복을 없애지 않으므로, 버전마다 `ordered_at`이 달라 다른 월로 넘어가면
/// 예전 행이 그 월에 남아 월 범위 리포트에서 이중 집계된다. 그래서 이전 행(`prev`)이 있으면
/// 그 `ordered_at`을 그대로 쓴다. 즉 한 주문의 `ordered_at`은 **처음 투영한 값으로 고정**된다
/// (확정 정정, 확정 전 환불·취소, 클라이언트 행을 S2S가 대체하는 경우 모두).
pub fn project(events: &[OrderEvent], prev: Option<&OrderState>) -> Option<OrderState> {
    let mut state = fold(events)?;
    if let Some(p) = prev {
        state.ordered_at_ms = p.ordered_at_ms;
    }
    Some(state)
}

fn amount(v: Option<i64>, field: &'static str) -> Result<Option<i64>, OrderError> {
    match v {
        Some(a) if !(0..=MAX_AMOUNT).contains(&a) => Err(OrderError::AmountOutOfRange(field)),
        other => Ok(other),
    }
}

/// 1~128자, 제어 문자 없음.
pub fn valid_order_id(s: &str) -> bool {
    let n = s.chars().count();
    (1..=MAX_ORDER_ID_CHARS).contains(&n) && !s.chars().any(char::is_control)
}

fn valid_idempotency_key(k: &str) -> bool {
    (1..=MAX_IDEMPOTENCY_KEY_LEN).contains(&k.len()) && k.bytes().all(|b| b.is_ascii_graphic())
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let t = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()?;
    i64::try_from(t.unix_timestamp_nanos() / 1_000_000).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SALT: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    const MEMBER: &str = "8d969eef6ecad3c29a3a629280e686cf0c3f5d5a86aff3ca12020c923adc6c92";
    /// 2026-11-20T09:45:12Z
    const ORDERED_MS: i64 = 1_795_167_912_000;
    /// 2026-11-21T00:00:00Z
    const NOW: i64 = 1_795_219_200_000;

    fn project() -> OrderProject<'static> {
        OrderProject {
            project_id: 7,
            currency: "KRW",
            salt_hex: SALT,
        }
    }

    fn confirm_json() -> serde_json::Value {
        json!({
            "event_type": "PURCHASE_CONFIRMED",
            "order_id": "ORD-20261120-009901",
            "visitor_id": "5c8a0d27-0861-4c76-b062-bc938f8c6833",
            "member_id_hash": MEMBER,
            "currency": "KRW",
            "gross_amount": 110000,
            "net_amount": 100000,
            "ordered_at": "2026-11-20T18:45:12+09:00",
            "items": [{ "id": "SKU1", "price": 100000, "qty": 1 }]
        })
    }

    fn ok(v: serde_json::Value) -> Validated {
        validate(&project(), &v, None, NOW).expect("유효해야 한다")
    }

    fn err(v: serde_json::Value) -> OrderError {
        validate(&project(), &v, None, NOW).expect_err("거부해야 한다")
    }

    fn with(mut v: serde_json::Value, k: &str, val: serde_json::Value) -> serde_json::Value {
        v[k] = val;
        v
    }

    fn ev(
        t: OrderEventType,
        key: &str,
        net: i64,
        refund: i64,
        occurred: i64,
        received: i64,
    ) -> OrderEvent {
        OrderEvent {
            project_id: 7,
            order_id: "O1".into(),
            event_type: t,
            source: OrderSource::S2s,
            idempotency_key: key.into(),
            visitor_id: None,
            member_hash: String::new(),
            currency: "KRW".into(),
            gross_amount: net,
            net_amount: net,
            refund_amount: refund,
            occurred_at_ms: occurred,
            received_at_ms: received,
            raw: "{}".into(),
        }
    }

    use OrderEventType::*;

    fn confirm(key: &str, net: i64, occurred: i64) -> OrderEvent {
        ev(PurchaseConfirmed, key, net, 0, occurred, occurred + 10)
    }
    fn refund(key: &str, amount: i64, occurred: i64) -> OrderEvent {
        ev(OrderRefunded, key, 0, amount, occurred, occurred + 10)
    }
    fn cancel(key: &str, occurred: i64) -> OrderEvent {
        ev(OrderCancelled, key, 0, 0, occurred, occurred + 10)
    }

    fn permutations(v: &[OrderEvent]) -> Vec<Vec<OrderEvent>> {
        if v.len() <= 1 {
            return vec![v.to_vec()];
        }
        let mut out = vec![];
        for i in 0..v.len() {
            let mut rest = v.to_vec();
            let x = rest.remove(i);
            for mut p in permutations(&rest) {
                p.insert(0, x.clone());
                out.push(p);
            }
        }
        out
    }

    // ── 해피 케이스 ──────────────────────────────────────────

    #[test]
    fn validates_manual_example_confirm() {
        let v = ok(confirm_json());
        let e = &v.event;
        assert!(v.warnings.is_empty());
        assert_eq!(e.project_id, 7);
        assert_eq!(e.order_id, "ORD-20261120-009901");
        assert_eq!(e.event_type, PurchaseConfirmed);
        assert_eq!(e.source, OrderSource::S2s);
        assert_eq!(e.net_amount, 100_000);
        assert_eq!(e.gross_amount, 110_000);
        assert_eq!(e.refund_amount, 0);
        assert_eq!(e.currency, "KRW");
        assert_eq!(e.occurred_at_ms, ORDERED_MS, "+09:00 → UTC ms");
        assert_eq!(e.received_at_ms, NOW);
        assert_eq!(
            e.visitor_id,
            Some(Uuid::parse_str("5c8a0d27-0861-4c76-b062-bc938f8c6833").unwrap())
        );
        assert_eq!(e.member_hash, rehash_member(SALT, MEMBER));
        assert_ne!(e.member_hash, MEMBER, "원본 해시를 그대로 저장하지 않는다");
        assert!(e.raw.contains("SKU1"), "원본 JSON 보관");
        assert_eq!(
            e.idempotency_key,
            default_idempotency_key(e.order_id.as_str(), PurchaseConfirmed, 100_000, ORDERED_MS)
        );
    }

    #[test]
    fn validates_cancel_and_refund() {
        let c = ok(
            json!({ "event_type": "ORDER_CANCELLED", "order_id": "ORD-1",
            "refund_amount": 100000, "cancelled_at": "2026-11-21T09:12:04+09:00" }),
        );
        assert_eq!(c.event.event_type, OrderCancelled);
        assert_eq!(c.event.refund_amount, 100_000);
        assert_eq!(c.event.currency, "KRW", "통화가 없으면 프로젝트 통화");
        let r = ok(json!({ "event_type": "ORDER_REFUNDED", "order_id": "ORD-1",
            "refund_amount": 30000, "refunded_at": "2026-11-20T12:00:00Z" }));
        assert_eq!(r.event.event_type, OrderRefunded);
        assert_eq!(r.event.refund_amount, 30_000);
        assert_eq!(r.event.net_amount, 0);
    }

    #[test]
    fn gross_defaults_to_net() {
        let mut v = confirm_json();
        v.as_object_mut().unwrap().remove("gross_amount");
        assert_eq!(ok(v).event.gross_amount, 100_000);
    }

    #[test]
    fn explicit_idempotency_key_wins() {
        // 명시 키는 이벤트 유형으로 네임스페이스를 붙여 저장한다
        let v = validate(&project(), &confirm_json(), Some("retry-abc-1"), NOW).unwrap();
        assert_eq!(v.event.idempotency_key, "PURCHASE_CONFIRMED:retry-abc-1");
        let body_key = with(confirm_json(), "idempotency_key", json!("body-key"));
        assert_eq!(
            ok(body_key).event.idempotency_key,
            "PURCHASE_CONFIRMED:body-key"
        );
    }

    #[test]
    fn explicit_key_reused_across_event_types_does_not_collide() {
        // 가맹점이 확정·환불 모두 `Idempotency-Key: <order_id>`를 보내는 경우
        let c = validate(&project(), &confirm_json(), Some("ORD-1"), NOW).unwrap();
        let r = json!({ "event_type": "ORDER_REFUNDED", "order_id": "ORD-20261120-009901",
            "refund_amount": 1000, "refunded_at": "2026-11-20T12:00:00Z" });
        let r = validate(&project(), &r, Some("ORD-1"), NOW).unwrap();
        assert_ne!(c.event.idempotency_key, r.event.idempotency_key);
        assert_eq!(r.event.idempotency_key, "ORDER_REFUNDED:ORD-1");
        let s = fold(&[c.event, r.event]).unwrap();
        assert_eq!(s.status, OrderStatus::PartiallyRefunded);
        assert_eq!(s.effective_amount, 99_000);
    }

    #[test]
    fn same_payload_detects_key_reuse_with_different_content() {
        let a = validate(&project(), &confirm_json(), Some("K"), NOW)
            .unwrap()
            .event;
        let mut b = a.clone();
        b.received_at_ms += 1000;
        b.raw = "{}".into();
        assert!(
            a.same_payload(&b),
            "재전송(수신 시각·원문만 다름)은 같은 요청"
        );
        let mut c = a.clone();
        c.net_amount -= 1;
        assert!(!a.same_payload(&c));
        let mut d = a.clone();
        d.occurred_at_ms += 1;
        assert!(!a.same_payload(&d));
        let mut e = a.clone();
        e.refund_amount = 5;
        assert!(!a.same_payload(&e));
    }

    #[test]
    fn fold_confirm_then_partial_refunds() {
        let s = fold(&[
            confirm("c", 100_000, 1_000),
            refund("r1", 30_000, 2_000),
            refund("r2", 20_000, 3_000),
        ])
        .unwrap();
        assert_eq!(s.status, OrderStatus::PartiallyRefunded);
        assert_eq!(s.net_amount, 100_000);
        assert_eq!(s.refunded_amount, 50_000);
        assert_eq!(s.effective_amount, 50_000);
        assert_eq!(s.ordered_at_ms, 1_000);
        assert_eq!(s.source, OrderSource::S2s);
    }

    #[test]
    fn fold_confirm_only() {
        let s = fold(&[confirm("c", 100_000, 1_000)]).unwrap();
        assert_eq!(s.status, OrderStatus::Confirmed);
        assert_eq!(s.effective_amount, 100_000);
        assert_eq!(s.refunded_amount, 0);
        assert!(fold(&[]).is_none());
    }

    #[test]
    fn fold_confirm_then_cancel() {
        let s = fold(&[confirm("c", 100_000, 1_000), cancel("x", 2_000)]).unwrap();
        assert_eq!(s.status, OrderStatus::Cancelled);
        assert_eq!(s.effective_amount, 0);
        assert_eq!(s.refunded_amount, 100_000);
        assert_eq!(s.net_amount, 100_000);
    }

    #[test]
    fn fold_carries_identity_from_confirm() {
        let mut c = confirm("c", 1, 1_000);
        c.visitor_id = Some(Uuid::from_u128(5));
        c.member_hash = "mh".into();
        let s = fold(&[refund("r", 1, 500), c]).unwrap();
        assert_eq!(s.visitor_id, Some(Uuid::from_u128(5)));
        assert_eq!(s.member_hash, "mh");
    }

    // ── 멱등성 ──────────────────────────────────────────────

    #[test]
    fn same_payload_same_default_key() {
        let a = ok(confirm_json()).event.idempotency_key;
        let b = validate(&project(), &confirm_json(), None, NOW + 5_000)
            .unwrap()
            .event
            .idempotency_key;
        assert_eq!(a, b, "재전송(다른 수신 시각)도 같은 키");
        // 같은 시각을 다른 오프셋으로 표현해도 같은 키
        let z = with(confirm_json(), "ordered_at", json!("2026-11-20T09:45:12Z"));
        assert_eq!(ok(z).event.idempotency_key, a);
        // 금액이 다르면 다른 키
        let other = with(confirm_json(), "net_amount", json!(99_999));
        assert_ne!(ok(other).event.idempotency_key, a);
    }

    #[test]
    fn default_key_is_sha256_hex() {
        let k = default_idempotency_key("O1", PurchaseConfirmed, 100, 5);
        assert_eq!(k.len(), 64);
        assert!(k.bytes().all(|b| b.is_ascii_hexdigit()));
        let expected = hex::encode(Sha256::digest(b"O1|PURCHASE_CONFIRMED|100|5"));
        assert_eq!(k, expected);
    }

    #[test]
    fn fold_dedupes_webhook_duplicates() {
        let r = refund("r1", 30_000, 2_000);
        let mut late_dup = r.clone();
        late_dup.received_at_ms += 86_400_000;
        let s = fold(&[
            confirm("c", 100_000, 1_000),
            r.clone(),
            r,
            late_dup,
            confirm("c", 100_000, 1_000),
        ])
        .unwrap();
        assert_eq!(s.refunded_amount, 30_000, "같은 키 환불은 한 번만");
        assert_eq!(s.effective_amount, 70_000);
    }

    #[test]
    fn fold_same_key_keeps_first_received() {
        // 같은 멱등 키로 내용이 다른 요청이 오면 먼저 받은 것을 쓴다.
        let first = ev(OrderRefunded, "k", 0, 10_000, 2_000, 2_100);
        let second = ev(OrderRefunded, "k", 0, 90_000, 2_000, 2_200);
        let base = confirm("c", 100_000, 1_000);
        let a = fold(&[base.clone(), second.clone(), first.clone()]).unwrap();
        let b = fold(&[base, first, second]).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.refunded_amount, 10_000);
    }

    // ── 경계값·엣지 ──────────────────────────────────────────

    #[test]
    fn amount_bounds() {
        assert_eq!(
            ok(with(confirm_json(), "net_amount", json!(0)))
                .event
                .net_amount,
            0
        );
        let max = with(
            with(confirm_json(), "net_amount", json!(MAX_AMOUNT)),
            "gross_amount",
            json!(MAX_AMOUNT),
        );
        assert_eq!(ok(max).event.net_amount, MAX_AMOUNT);
        assert_eq!(
            err(with(confirm_json(), "net_amount", json!(MAX_AMOUNT + 1))),
            OrderError::AmountOutOfRange("net_amount")
        );
        assert_eq!(
            err(with(confirm_json(), "net_amount", json!(-1))),
            OrderError::AmountOutOfRange("net_amount")
        );
        assert_eq!(
            err(with(confirm_json(), "gross_amount", json!(-5))),
            OrderError::AmountOutOfRange("gross_amount")
        );
        let neg_refund = json!({ "event_type": "ORDER_REFUNDED", "order_id": "O",
            "refund_amount": -1, "refunded_at": "2026-11-20T12:00:00Z" });
        assert_eq!(
            err(neg_refund),
            OrderError::AmountOutOfRange("refund_amount")
        );
    }

    #[test]
    fn non_integer_amounts_rejected() {
        assert!(matches!(
            err(with(confirm_json(), "net_amount", json!(100.5))),
            OrderError::InvalidPayload(_)
        ));
        assert!(matches!(
            err(with(confirm_json(), "net_amount", json!("100000"))),
            OrderError::InvalidPayload(_)
        ));
        assert!(matches!(
            err(with(confirm_json(), "net_amount", json!(1e30))),
            OrderError::InvalidPayload(_)
        ));
    }

    #[test]
    fn order_id_length_bounds() {
        let id128 = "가".repeat(128);
        assert_eq!(
            ok(with(confirm_json(), "order_id", json!(id128)))
                .event
                .order_id
                .chars()
                .count(),
            128,
            "128자(멀티바이트)까지 허용"
        );
        assert_eq!(
            err(with(confirm_json(), "order_id", json!("a".repeat(129)))),
            OrderError::InvalidOrderId
        );
        assert_eq!(
            err(with(confirm_json(), "order_id", json!(""))),
            OrderError::InvalidOrderId
        );
    }

    #[test]
    fn unicode_order_id_kept_verbatim() {
        let id = "주문-🛒-ñ-001";
        assert_eq!(
            ok(with(confirm_json(), "order_id", json!(id)))
                .event
                .order_id,
            id
        );
    }

    #[test]
    fn missing_required_fields() {
        let mut v = confirm_json();
        v.as_object_mut().unwrap().remove("net_amount");
        assert_eq!(err(v), OrderError::MissingField("net_amount"));
        let mut v = confirm_json();
        v.as_object_mut().unwrap().remove("ordered_at");
        assert_eq!(err(v), OrderError::MissingField("ordered_at"));
        let r = json!({ "event_type": "ORDER_REFUNDED", "order_id": "O", "refunded_at": "2026-11-20T12:00:00Z" });
        assert_eq!(err(r), OrderError::MissingField("refund_amount"));
        let c = json!({ "event_type": "ORDER_CANCELLED", "order_id": "O" });
        assert_eq!(err(c), OrderError::MissingField("cancelled_at"));
        // 취소는 금액 없이도 된다 (전액 취소)
        let c = json!({ "event_type": "ORDER_CANCELLED", "order_id": "O", "cancelled_at": "2026-11-20T12:00:00Z" });
        assert_eq!(ok(c).event.refund_amount, 0);
        let mut v = confirm_json();
        v.as_object_mut().unwrap().remove("order_id");
        assert!(matches!(err(v), OrderError::InvalidPayload(_)));
    }

    #[test]
    fn unknown_event_type() {
        assert_eq!(
            err(with(
                confirm_json(),
                "event_type",
                json!("purchase_confirmed")
            )),
            OrderError::UnknownEventType
        );
        assert_eq!(
            err(with(confirm_json(), "event_type", json!(""))),
            OrderError::UnknownEventType
        );
    }

    #[test]
    fn currency_rules() {
        assert_eq!(
            err(with(confirm_json(), "currency", json!("USD"))),
            OrderError::CurrencyMismatch
        );
        assert_eq!(
            ok(with(confirm_json(), "currency", json!("krw")))
                .event
                .currency,
            "KRW",
            "대소문자 무시"
        );
        let usd = OrderProject {
            currency: "USD",
            ..project()
        };
        let v = validate(&usd, &confirm_json(), None, NOW);
        assert_eq!(v.unwrap_err(), OrderError::CurrencyMismatch);
    }

    #[test]
    fn timestamp_rules() {
        let at = |s: &str| with(confirm_json(), "ordered_at", json!(s));
        assert_eq!(
            err(at("2026-11-20 18:45:12")),
            OrderError::InvalidTimestamp("ordered_at")
        );
        assert_eq!(
            err(at("1763631912")),
            OrderError::InvalidTimestamp("ordered_at")
        );
        assert_eq!(err(at("")), OrderError::InvalidTimestamp("ordered_at"));
        assert_eq!(
            err(at("1999-12-31T23:59:59Z")),
            OrderError::TimestampOutOfRange("ordered_at")
        );
        assert_eq!(
            ok(at("2000-01-01T00:00:00Z")).event.occurred_at_ms,
            MIN_OCCURRED_AT_MS
        );
        // 현재+1일까지 허용, 넘으면 거부
        assert!(validate(&project(), &at("2026-11-22T00:00:00Z"), None, NOW).is_ok());
        assert_eq!(
            err(at("2026-11-22T00:00:00.001Z")),
            OrderError::TimestampOutOfRange("ordered_at")
        );
        // 밀리초 보존
        assert_eq!(
            ok(at("2026-11-20T09:45:12.345Z")).event.occurred_at_ms,
            ORDERED_MS + 345
        );
    }

    #[test]
    fn visitor_id_invalid_is_warning() {
        for bad in [
            json!("not-a-uuid"),
            json!("00000000-0000-0000-0000-000000000000"),
            json!(12),
        ] {
            let v = with(confirm_json(), "visitor_id", bad.clone());
            match validate(&project(), &v, None, NOW) {
                Ok(v) => {
                    assert_eq!(v.event.visitor_id, None, "{bad}");
                    assert_eq!(v.warnings, vec![WARN_INVALID_VISITOR_ID], "{bad}");
                }
                Err(OrderError::InvalidPayload(_)) if bad.is_number() => {}
                Err(e) => panic!("{bad}: {e}"),
            }
        }
        let empty = with(confirm_json(), "visitor_id", json!(""));
        let v = ok(empty);
        assert_eq!(v.event.visitor_id, None);
        assert!(v.warnings.is_empty(), "빈 값은 경고 없이 없음");
    }

    #[test]
    fn fold_zero_amount_order() {
        let s = fold(&[confirm("c", 0, 1_000)]).unwrap();
        assert_eq!(s.status, OrderStatus::Confirmed);
        assert_eq!(s.effective_amount, 0);
    }

    #[test]
    fn fold_refund_exceeding_net_floors_at_zero() {
        let s = fold(&[
            confirm("c", 100, 1),
            refund("r1", 70, 2),
            refund("r2", 70, 3),
        ])
        .unwrap();
        assert_eq!(s.effective_amount, 0);
        assert_eq!(s.refunded_amount, 140);
        assert_eq!(
            s.status,
            OrderStatus::Cancelled,
            "전액 이상 환불 = 취소와 같음"
        );
    }

    #[test]
    fn fold_max_amount_no_overflow() {
        let mut evs = vec![confirm("c", MAX_AMOUNT, 1)];
        for i in 0..1000 {
            evs.push(refund(&format!("r{i}"), MAX_AMOUNT, 2 + i));
        }
        let s = fold(&evs).unwrap();
        assert_eq!(s.effective_amount, 0);
        assert_eq!(s.refunded_amount, MAX_AMOUNT * 1000);
    }

    #[test]
    fn fold_large_event_count() {
        let mut evs = vec![confirm("c", 10_000_000, 1)];
        for i in 0..5_000 {
            evs.push(refund(&format!("r{i}"), 1, 10 + i));
        }
        let s = fold(&evs).unwrap();
        assert_eq!(s.refunded_amount, 5_000);
        assert_eq!(s.effective_amount, 10_000_000 - 5_000);
    }

    // ── 희귀 케이스 ──────────────────────────────────────────

    #[test]
    fn cancel_before_confirm_stays_cancelled() {
        // 취소가 먼저 도착(그리고 먼저 발생)해도, 나중 확정이 상태를 되돌리지 않는다.
        let c = cancel("x", 1_000);
        let p = confirm("c", 100_000, 2_000);
        for order in permutations(&[c, p]) {
            let s = fold(&order).unwrap();
            assert_eq!(s.status, OrderStatus::Cancelled);
            assert_eq!(s.effective_amount, 0);
            assert_eq!(s.net_amount, 100_000);
            assert_eq!(s.ordered_at_ms, 2_000);
        }
    }

    #[test]
    fn cancel_only_order() {
        let s = fold(&[cancel("x", 5_000)]).unwrap();
        assert_eq!(s.status, OrderStatus::Cancelled);
        assert_eq!(s.net_amount, 0);
        assert_eq!(s.effective_amount, 0);
        assert_eq!(s.ordered_at_ms, 5_000);
    }

    #[test]
    fn refund_only_is_pending() {
        let s = fold(&[refund("r", 3_000, 5_000)]).unwrap();
        assert_eq!(s.status, OrderStatus::Pending);
        assert_eq!(s.effective_amount, 0);
        assert_eq!(s.refunded_amount, 3_000);
    }

    #[test]
    fn out_of_order_partial_refunds_any_permutation() {
        let evs = [
            confirm("c", 100_000, 1_000),
            refund("r1", 10_000, 4_000),
            refund("r2", 20_000, 2_000),
            refund("r3", 5_000, 3_000),
        ];
        let expected = fold(&evs).unwrap();
        assert_eq!(expected.effective_amount, 65_000);
        for p in permutations(&evs) {
            assert_eq!(fold(&p).unwrap(), expected);
        }
    }

    #[test]
    fn same_ms_events_are_deterministic() {
        // 같은 ms의 확정 두 건(금액 정정) + 환불: 입력 순서와 관계없이 같은 결과.
        let a = ev(PurchaseConfirmed, "a", 100_000, 0, 1_000, 1_000);
        let b = ev(PurchaseConfirmed, "b", 90_000, 0, 1_000, 1_000);
        let r = ev(OrderRefunded, "r", 0, 10_000, 1_000, 1_000);
        let expected = fold(&[a.clone(), b.clone(), r.clone()]).unwrap();
        for p in permutations(&[a, b, r]) {
            assert_eq!(fold(&p).unwrap(), expected);
        }
        assert_eq!(
            expected.net_amount, 90_000,
            "동률이면 멱등 키 순으로 마지막 확정"
        );
        assert_eq!(expected.effective_amount, 80_000);
    }

    #[test]
    fn later_confirm_corrects_amount() {
        let s = fold(&[confirm("c2", 80_000, 2_000), confirm("c1", 100_000, 1_000)]).unwrap();
        assert_eq!(s.net_amount, 80_000);
        assert_eq!(s.ordered_at_ms, 2_000);
        assert_eq!(s.status, OrderStatus::Confirmed);
    }

    #[test]
    fn project_freezes_ordered_at_of_previous_row() {
        // orders는 toYYYYMM(ordered_at)로 파티션된다. 이전 행의 ordered_at을 유지해야
        // 같은 주문의 버전들이 한 파티션에 남아 FINAL이 중복을 없앤다.
        let first = [refund("r", 1_000, 5_000)];
        let prev = super::project(&first, None).unwrap();
        assert_eq!(prev.ordered_at_ms, 5_000);
        let more = [refund("r", 1_000, 5_000), confirm("c", 100_000, 1_000)];
        assert_eq!(
            fold(&more).unwrap().ordered_at_ms,
            1_000,
            "fold 자체는 확정 시각"
        );
        let next = super::project(&more, Some(&prev)).unwrap();
        assert_eq!(next.ordered_at_ms, 5_000, "투영은 이전 행의 시각을 유지");
        assert_eq!(next.status, OrderStatus::PartiallyRefunded);
        // 확정 정정도 마찬가지
        let a = super::project(&[confirm("c1", 100_000, 1_000)], None).unwrap();
        let b = super::project(
            &[confirm("c1", 100_000, 1_000), confirm("c2", 90_000, 9_000)],
            Some(&a),
        )
        .unwrap();
        assert_eq!((b.net_amount, b.ordered_at_ms), (90_000, 1_000));
        assert!(super::project(&[], Some(&a)).is_none());
    }

    #[test]
    fn client_only_order_keeps_first_confirm() {
        // 공개 키(pk)만 있으면 누구나 purchase를 보낼 수 있으므로, 클라이언트 주문은 첫 확정을 쓴다.
        let mut first = confirm("client:1", 50_000, 1_000);
        first.source = OrderSource::Client;
        first.visitor_id = Some(Uuid::from_u128(1));
        let mut forged = confirm("client:2", 1, 2_000);
        forged.source = OrderSource::Client;
        forged.visitor_id = Some(Uuid::from_u128(2));
        for p in permutations(&[first.clone(), forged.clone()]) {
            let s = fold(&p).unwrap();
            assert_eq!(
                s.net_amount, 50_000,
                "나중 클라이언트 확정이 금액을 바꾸지 못한다"
            );
            assert_eq!(s.ordered_at_ms, 1_000);
            assert_eq!(s.visitor_id, Some(Uuid::from_u128(1)));
        }
        // S2S가 오면 S2S의 최신 확정이 이긴다
        let s2s = [confirm("a", 100_000, 3_000), confirm("b", 90_000, 4_000)];
        let all = [first, forged, s2s[0].clone(), s2s[1].clone()];
        let s = fold(&all).unwrap();
        assert_eq!((s.source, s.net_amount), (OrderSource::S2s, 90_000));
    }

    #[test]
    fn s2s_wins_over_client() {
        let mut client = confirm("client:1", 120_000, 500);
        client.source = OrderSource::Client;
        client.visitor_id = Some(Uuid::from_u128(9));
        let s2s = confirm("c", 100_000, 1_000);
        for p in permutations(&[client.clone(), s2s]) {
            let s = fold(&p).unwrap();
            assert_eq!(s.source, OrderSource::S2s);
            assert_eq!(s.net_amount, 100_000);
        }
        let only_client = fold(&[client]).unwrap();
        assert_eq!(only_client.source, OrderSource::Client);
        assert_eq!(only_client.net_amount, 120_000);
        assert_eq!(only_client.visitor_id, Some(Uuid::from_u128(9)));
    }

    // ── 외부 공격 ────────────────────────────────────────────

    #[test]
    fn control_chars_in_order_id_rejected() {
        for bad in ["ORD\n1", "ORD\u{0}1", "ORD\t1", "\u{7f}", "ORD\r\n"] {
            assert_eq!(
                err(with(confirm_json(), "order_id", json!(bad))),
                OrderError::InvalidOrderId,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn sql_like_order_id_kept_verbatim() {
        let id = "x'; DROP TABLE orders; -- ? \\ \"";
        assert_eq!(
            ok(with(confirm_json(), "order_id", json!(id)))
                .event
                .order_id,
            id
        );
    }

    #[test]
    fn plain_member_id_rejected() {
        for bad in [
            "user@example.com",
            "010-1234-5678",
            "01012345678",
            "8d969eef",
            &"z".repeat(64),
            &format!("{MEMBER}0"),
        ] {
            assert_eq!(
                err(with(confirm_json(), "member_id_hash", json!(bad))),
                OrderError::InvalidMemberHash,
                "{bad}"
            );
        }
        // 대문자 hex는 허용하고 소문자로 정규화해 재해싱
        let upper = ok(with(
            confirm_json(),
            "member_id_hash",
            json!(MEMBER.to_uppercase()),
        ));
        assert_eq!(upper.event.member_hash, rehash_member(SALT, MEMBER));
        // 빈 값은 없음
        let empty = ok(with(confirm_json(), "member_id_hash", json!("")));
        assert_eq!(empty.event.member_hash, "");
    }

    #[test]
    fn member_hash_differs_per_project_salt() {
        let other_salt = "ff".repeat(32);
        let p2 = OrderProject {
            salt_hex: &other_salt,
            ..project()
        };
        let a = ok(confirm_json()).event.member_hash;
        let b = validate(&p2, &confirm_json(), None, NOW)
            .unwrap()
            .event
            .member_hash;
        assert_ne!(a, b);
    }

    #[test]
    fn invalid_idempotency_keys_rejected() {
        let long = "k".repeat(MAX_IDEMPOTENCY_KEY_LEN + 1);
        for bad in ["", "a\nb", "키", long.as_str()] {
            assert_eq!(
                validate(&project(), &confirm_json(), Some(bad), NOW).unwrap_err(),
                OrderError::InvalidIdempotencyKey,
                "{bad:?}"
            );
        }
        let max = "k".repeat(MAX_IDEMPOTENCY_KEY_LEN);
        assert!(validate(&project(), &confirm_json(), Some(&max), NOW).is_ok());
    }

    #[test]
    fn non_object_payloads_rejected() {
        for bad in [json!(null), json!([]), json!("x"), json!(1), json!({})] {
            assert!(
                matches!(err(bad.clone()), OrderError::InvalidPayload(_)),
                "{bad}"
            );
        }
    }

    // ── 클라이언트 purchase → 주문 ──────────────────────────

    fn purchase_event(order_id: &str, value: i64, currency: &str) -> Event {
        let raw = json!({
            "project_id": 7, "event_id": Uuid::from_u128(1), "ts_ms": 1_000, "received_at_ms": 1_050,
            "event_name": "purchase", "visitor_id": Uuid::from_u128(2), "session_id": Uuid::from_u128(3),
            "is_session_start": false, "member_hash": "mh",
            "url": "https://a.com/done", "host": "a.com", "path": "/done", "title": "", "referrer": "",
            "referrer_host": "", "utm_source": "", "utm_medium": "", "utm_campaign": "", "utm_content": "",
            "utm_term": "", "click_id": "", "click_id_type": "", "channel": "direct", "country": "KR",
            "region": "", "city": "", "device_type": "desktop", "browser": "", "os": "", "screen_w": 0,
            "props": [], "ec": { "order_id": order_id, "value": value, "currency": currency, "items": [] }
        });
        serde_json::from_value(raw).unwrap()
    }

    #[test]
    fn client_purchase_projects_as_client_order() {
        let e = client_purchase_event("KRW", &purchase_event("ORD-1", 50_000, "KRW")).unwrap();
        assert_eq!(e.source, OrderSource::Client);
        assert_eq!(e.event_type, PurchaseConfirmed);
        assert_eq!(e.order_id, "ORD-1");
        assert_eq!(e.net_amount, 50_000);
        assert_eq!(e.gross_amount, 50_000);
        assert_eq!(e.visitor_id, Some(Uuid::from_u128(2)));
        assert_eq!(e.member_hash, "mh");
        assert_eq!(e.occurred_at_ms, 1_000);
        assert_eq!(e.received_at_ms, 1_050);
        // 트래커 재전송(같은 event_id)은 같은 키
        let again = client_purchase_event("KRW", &purchase_event("ORD-1", 50_000, "KRW")).unwrap();
        assert_eq!(e.idempotency_key, again.idempotency_key);
    }

    #[test]
    fn client_purchase_skips_invalid() {
        assert!(client_purchase_event("KRW", &purchase_event("", 1, "KRW")).is_none());
        assert!(client_purchase_event("KRW", &purchase_event("O", 1, "USD")).is_none());
        assert!(client_purchase_event("KRW", &purchase_event("O", -1, "KRW")).is_none());
        assert!(client_purchase_event("KRW", &purchase_event("O\n", 1, "KRW")).is_none());
        let mut pv = purchase_event("O", 1, "KRW");
        pv.event_name = "page_view".into();
        assert!(client_purchase_event("KRW", &pv).is_none());
        let mut no_ec = purchase_event("O", 1, "KRW");
        no_ec.ec = None;
        assert!(client_purchase_event("KRW", &no_ec).is_none());
    }
}

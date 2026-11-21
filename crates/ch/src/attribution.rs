//! 기여 연산 잡용 쿼리 (ARCHITECTURE §7).
//!
//! - 주문: `orders FINAL`의 현재 상태 (WP1이 쓰는 테이블, `version` 단조 증가)
//! - 식별자 확장: `identity_links` (member_hash → visitor_id들)
//! - 터치포인트: `touchpoints` 원본 값 (채널은 잡이 현재 규칙으로 다시 계산한다)
//! - 결과: `attributed_revenue` (ReplacingMergeTree(computed_at, is_deleted))
//!
//! 시각은 모두 UTC epoch 밀리초(`i64`)로 주고받는다. `DateTime64(3)`의 RowBinary 표현과 같다.

use crate::ProjectScope;
use analytics_core::orders::OrderStatus;
use clickhouse::{Client, Row};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 한 쿼리에 넣는 IN 목록 최대 길이 (max_query_size 256KiB 안쪽).
pub const IN_CHUNK: usize = 1_000;

/// 기여 연산 입력이 되는 주문 현재 상태.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct OrderForAttribution {
    pub order_id: String,
    /// 읽을 때 [`OrderStatus`]로 파싱한다 (모르는 값이면 행 읽기 오류).
    #[serde(with = "status_str")]
    pub status: OrderStatus,
    #[serde(with = "clickhouse::serde::uuid::option")]
    pub visitor_id: Option<Uuid>,
    pub member_hash: String,
    pub effective_amount: i64,
    pub ordered_at_ms: i64,
    pub version: u64,
}

/// `orders.status` 문자열 ↔ [`OrderStatus`].
mod status_str {
    use analytics_core::orders::OrderStatus;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(s: &OrderStatus, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(s.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<OrderStatus, D::Error> {
        let s = String::deserialize(d)?;
        OrderStatus::parse(&s)
            .ok_or_else(|| D::Error::custom(format!("알 수 없는 주문 상태 {s:?}")))
    }
}

/// identity_links 한 줄.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct IdentityLink {
    pub member_hash: String,
    #[serde(with = "clickhouse::serde::uuid")]
    pub visitor_id: Uuid,
}

/// 터치포인트 원본 값 (채널 재계산용).
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct TouchRow {
    #[serde(with = "clickhouse::serde::uuid")]
    pub visitor_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    pub session_id: Uuid,
    pub ts_ms: i64,
    pub utm_source: String,
    pub utm_medium: String,
    pub utm_campaign: String,
    pub click_id_type: String,
    pub referrer_host: String,
    pub page_host: String,
}

/// `attributed_revenue` 행. 시각은 epoch ms. 쓰기([`insert_rows`])와 기존 결과 읽기
/// ([`latest_rows`]: 키별 최신 행, 삭제 표시 포함)에 같이 쓴다.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct AttributedRow {
    pub project_id: u32,
    pub model: String,
    pub lookback_days: u8,
    pub order_id: String,
    pub touch_index: u16,
    pub channel: String,
    pub utm_campaign: String,
    pub touch_ts: Option<i64>,
    pub ordered_at: i64,
    pub amount: i64,
    pub is_assist: u8,
    pub computed_at: i64,
    pub is_deleted: u8,
}

/// 매출 보존 법칙(§7.4)을 어긴 주문.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct BrokenOrder {
    pub model: String,
    pub lookback_days: u8,
    pub order_id: String,
    pub attributed: i64,
    pub effective_amount: i64,
}

const ORDER_COLS: &str = "SELECT order_id, status, visitor_id, member_hash, effective_amount, \
     toUnixTimestamp64Milli(ordered_at) AS ordered_at_ms, version \
     FROM orders FINAL WHERE project_id = ?";

/// `?, ?, ?` (n개)
fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// `version > since_version`인 주문 (version 오름차순).
pub async fn changed_orders(
    client: &Client,
    scope: ProjectScope,
    since_version: u64,
) -> anyhow::Result<Vec<OrderForAttribution>> {
    Ok(client
        .query(&format!(
            "{ORDER_COLS} AND version > ? ORDER BY version, order_id"
        ))
        .bind(scope.project_id())
        .bind(since_version)
        .fetch_all()
        .await?)
}

/// `ordered_at >= from_ms`인 주문 (ordered_at 오름차순).
pub async fn orders_since(
    client: &Client,
    scope: ProjectScope,
    from_ms: i64,
) -> anyhow::Result<Vec<OrderForAttribution>> {
    Ok(client
        .query(&format!(
            "{ORDER_COLS} AND ordered_at >= fromUnixTimestamp64Milli(toInt64(?), 'UTC') \
             ORDER BY ordered_at, order_id"
        ))
        .bind(scope.project_id())
        .bind(from_ms)
        .fetch_all()
        .await?)
}

/// 주문 ID로 조회한다 (없는 ID는 무시).
pub async fn orders_by_ids(
    client: &Client,
    scope: ProjectScope,
    order_ids: &[String],
) -> anyhow::Result<Vec<OrderForAttribution>> {
    let mut out = Vec::new();
    for chunk in order_ids.chunks(IN_CHUNK) {
        let mut q = client
            .query(&format!(
                "{ORDER_COLS} AND order_id IN ({}) ORDER BY order_id",
                placeholders(chunk.len())
            ))
            .bind(scope.project_id());
        for id in chunk {
            q = q.bind(id.as_str());
        }
        out.extend(q.fetch_all::<OrderForAttribution>().await?);
    }
    Ok(out)
}

/// 회원 해시들에 연결된 visitor_id. 빈 해시는 무시한다.
pub async fn identity_links(
    client: &Client,
    scope: ProjectScope,
    member_hashes: &[String],
) -> anyhow::Result<Vec<IdentityLink>> {
    let mut hashes: Vec<&str> = member_hashes
        .iter()
        .map(String::as_str)
        .filter(|h| !h.is_empty())
        .collect();
    hashes.sort_unstable();
    hashes.dedup();
    let mut out = Vec::new();
    for chunk in hashes.chunks(IN_CHUNK) {
        let mut q = client
            .query(&format!(
                "SELECT DISTINCT member_hash, visitor_id FROM identity_links \
                 WHERE project_id = ? AND member_hash IN ({}) ORDER BY member_hash, visitor_id",
                placeholders(chunk.len())
            ))
            .bind(scope.project_id());
        for h in chunk {
            q = q.bind(*h);
        }
        out.extend(q.fetch_all::<IdentityLink>().await?);
    }
    Ok(out)
}

/// visitor_id 집합의 `[from_ms, to_ms]` 터치포인트. (ts, session_id) 순.
pub async fn touchpoints(
    client: &Client,
    scope: ProjectScope,
    visitor_ids: &[Uuid],
    from_ms: i64,
    to_ms: i64,
) -> anyhow::Result<Vec<TouchRow>> {
    let mut ids = visitor_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut out = Vec::new();
    for chunk in ids.chunks(IN_CHUNK) {
        let mut q = client
            .query(&format!(
                "SELECT visitor_id, session_id, toUnixTimestamp64Milli(ts) AS ts_ms, \
                        utm_source, utm_medium, utm_campaign, click_id_type, referrer_host, page_host \
                 FROM touchpoints \
                 WHERE project_id = ? AND visitor_id IN ({}) \
                   AND ts >= fromUnixTimestamp64Milli(toInt64(?), 'UTC') \
                   AND ts <= fromUnixTimestamp64Milli(toInt64(?), 'UTC')",
                placeholders(chunk.len())
            ))
            .bind(scope.project_id());
        for v in chunk {
            // UUID 문자열 리터럴은 IN 집합에서 UUID로 변환된다.
            q = q.bind(v.to_string());
        }
        out.extend(q.bind(from_ms).bind(to_ms).fetch_all::<TouchRow>().await?);
    }
    out.sort_by(|a, b| {
        (a.ts_ms, a.session_id, a.visitor_id).cmp(&(b.ts_ms, b.session_id, b.visitor_id))
    });
    Ok(out)
}

/// 주문들의 기존 기여 결과: 키(model, lookback_days, order_id, touch_index, ordered_at)별 최신 행.
/// 삭제 표시된 행도 포함한다 (`computed_at`을 단조 증가시키기 위해).
///
/// FINAL은 삭제 표시 행을 숨기므로, `computed_at` 최댓값을 알기 위해 argMax로 직접 접는다.
/// 열 순서는 [`AttributedRow`] 필드 순서와 같다.
pub async fn latest_rows(
    client: &Client,
    scope: ProjectScope,
    order_ids: &[String],
) -> anyhow::Result<Vec<AttributedRow>> {
    let mut out = Vec::new();
    for chunk in order_ids.chunks(IN_CHUNK) {
        let mut q = client
            .query(&format!(
                "SELECT toUInt32(?) AS project_id, k_model AS model, k_lb AS lookback_days, \
                        k_order AS order_id, k_idx AS touch_index, \
                        argMax(v_channel, v_c) AS channel, \
                        argMax(v_campaign, v_c) AS utm_campaign, \
                        argMax(tuple(v_touch_ts), v_c).1 AS touch_ts, \
                        k_oa AS ordered_at, \
                        argMax(v_amount, v_c) AS amount, \
                        argMax(v_assist, v_c) AS is_assist, \
                        max(v_c) AS computed_at, \
                        argMax(v_deleted, v_c) AS is_deleted \
                 FROM ( \
                   SELECT order_id AS k_order, model AS k_model, lookback_days AS k_lb, \
                          touch_index AS k_idx, toUnixTimestamp64Milli(ordered_at) AS k_oa, \
                          channel AS v_channel, utm_campaign AS v_campaign, \
                          toUnixTimestamp64Milli(touch_ts) AS v_touch_ts, amount AS v_amount, \
                          is_assist AS v_assist, is_deleted AS v_deleted, \
                          toUnixTimestamp64Milli(computed_at) AS v_c \
                   FROM attributed_revenue \
                   WHERE project_id = ? AND order_id IN ({}) \
                 ) \
                 GROUP BY k_order, k_model, k_lb, k_idx, k_oa \
                 ORDER BY k_order, k_model, k_lb, k_idx, k_oa",
                placeholders(chunk.len())
            ))
            .bind(scope.project_id())
            .bind(scope.project_id());
        for id in chunk {
            q = q.bind(id.as_str());
        }
        out.extend(q.fetch_all::<AttributedRow>().await?);
    }
    Ok(out)
}

/// 기여 결과를 한 번의 INSERT로 쓴다. 모든 행은 `scope` 프로젝트여야 한다.
pub async fn insert_rows(
    client: &Client,
    scope: ProjectScope,
    rows: &[AttributedRow],
) -> anyhow::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    anyhow::ensure!(
        rows.iter().all(|r| r.project_id == scope.project_id()),
        "다른 프로젝트의 기여 행은 쓸 수 없다"
    );
    let mut ins = client.insert::<AttributedRow>("attributed_revenue").await?;
    for r in rows {
        ins.write(r).await?;
    }
    ins.end().await?;
    Ok(())
}

/// 매출 보존 검증 (§7.4): raw_sum 외 모델에서 Σamount ≠ effective_amount인 주문.
pub async fn conservation_check(
    client: &Client,
    scope: ProjectScope,
) -> anyhow::Result<Vec<BrokenOrder>> {
    Ok(client
        .query(
            "SELECT a.model AS model, a.lookback_days AS lookback_days, a.order_id AS order_id, \
                    a.s AS attributed, o.effective_amount AS effective_amount \
             FROM ( \
               SELECT model, lookback_days, order_id, sum(amount) AS s \
               FROM attributed_revenue FINAL \
               WHERE project_id = ? AND model != 'raw_sum' AND is_deleted = 0 \
               GROUP BY model, lookback_days, order_id \
             ) AS a \
             INNER JOIN ( \
               SELECT order_id, effective_amount FROM orders FINAL WHERE project_id = ? \
             ) AS o ON a.order_id = o.order_id \
             WHERE a.s != o.effective_amount \
             ORDER BY order_id, model, lookback_days",
        )
        .bind(scope.project_id())
        .bind(scope.project_id())
        .fetch_all()
        .await?)
}

/// 테스트 픽스처: 다른 패키지(WP1 주문 writer, 수집 MV)가 쓰는 테이블에 행을 직접 넣는다.
/// `fixtures` 기능을 켠 빌드(이 크레이트·jobs의 테스트)에서만 컴파일된다.
#[cfg(feature = "fixtures")]
#[doc(hidden)]
pub mod fixtures {
    use clickhouse::{Client, Row, RowOwned, RowWrite};
    use serde::Serialize;
    use uuid::Uuid;

    /// `orders` 최소 행 (테이블 계약: ReplacingMergeTree(version), FINAL로 읽음).
    #[derive(Debug, Clone, Row, Serialize)]
    pub struct OrderFixture {
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
        pub ordered_at: i64,
        pub version: u64,
    }

    impl OrderFixture {
        /// 확정 주문.
        pub fn confirmed(
            project_id: u32,
            order_id: &str,
            visitor_id: Option<Uuid>,
            amount: i64,
            ordered_at: i64,
            version: u64,
        ) -> Self {
            Self {
                project_id,
                order_id: order_id.into(),
                source: "s2s".into(),
                status: analytics_core::orders::OrderStatus::Confirmed
                    .as_str()
                    .into(),
                visitor_id,
                member_hash: String::new(),
                currency: "KRW".into(),
                gross_amount: amount,
                net_amount: amount,
                refunded_amount: 0,
                effective_amount: amount,
                ordered_at,
                version,
            }
        }
    }

    /// `touchpoints` 행.
    #[derive(Debug, Clone, Row, Serialize)]
    pub struct TouchFixture {
        pub project_id: u32,
        #[serde(with = "clickhouse::serde::uuid")]
        pub visitor_id: Uuid,
        pub member_hash: String,
        pub ts: i64,
        #[serde(with = "clickhouse::serde::uuid")]
        pub session_id: Uuid,
        pub utm_source: String,
        pub utm_medium: String,
        pub utm_campaign: String,
        pub utm_content: String,
        pub utm_term: String,
        pub click_id: String,
        pub click_id_type: String,
        pub referrer_host: String,
        pub page_host: String,
        pub landing_url: String,
        /// 수집 시점 채널 (기여 연산은 이 값을 쓰지 않고 다시 계산한다)
        pub channel: String,
    }

    impl TouchFixture {
        /// utm 유입.
        pub fn utm(
            project_id: u32,
            visitor_id: Uuid,
            ts: i64,
            source: &str,
            medium: &str,
            campaign: &str,
        ) -> Self {
            Self {
                project_id,
                visitor_id,
                member_hash: String::new(),
                ts,
                session_id: Uuid::new_v4(),
                utm_source: source.into(),
                utm_medium: medium.into(),
                utm_campaign: campaign.into(),
                utm_content: String::new(),
                utm_term: String::new(),
                click_id: String::new(),
                click_id_type: String::new(),
                referrer_host: String::new(),
                page_host: "shop.example.com".into(),
                landing_url: "https://shop.example.com/".into(),
                channel: "stale_ingest_channel".into(),
            }
        }

        /// referrer 유입 (utm 없음). 빈 문자열이면 direct.
        pub fn referrer(project_id: u32, visitor_id: Uuid, ts: i64, referrer_host: &str) -> Self {
            let mut t = Self::utm(project_id, visitor_id, ts, "", "", "");
            t.referrer_host = referrer_host.into();
            t
        }
    }

    /// `identity_links` 행.
    #[derive(Debug, Clone, Row, Serialize)]
    pub struct IdentityFixture {
        pub project_id: u32,
        pub member_hash: String,
        #[serde(with = "clickhouse::serde::uuid")]
        pub visitor_id: Uuid,
        pub first_seen: i64,
        pub last_seen: i64,
    }

    impl IdentityFixture {
        pub fn new(project_id: u32, member_hash: &str, visitor_id: Uuid) -> Self {
            Self {
                project_id,
                member_hash: member_hash.into(),
                visitor_id,
                first_seen: 0,
                last_seen: 0,
            }
        }
    }

    pub async fn insert_orders(client: &Client, rows: &[OrderFixture]) -> anyhow::Result<()> {
        insert(client, "orders", rows).await
    }

    pub async fn insert_touches(client: &Client, rows: &[TouchFixture]) -> anyhow::Result<()> {
        insert(client, "touchpoints", rows).await
    }

    pub async fn insert_links(client: &Client, rows: &[IdentityFixture]) -> anyhow::Result<()> {
        insert(client, "identity_links", rows).await
    }

    async fn insert<T: RowOwned + RowWrite>(
        client: &Client,
        table: &str,
        rows: &[T],
    ) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut ins = client.insert::<T>(table).await?;
        for r in rows {
            ins.write(r).await?;
        }
        ins.end().await?;
        Ok(())
    }
}

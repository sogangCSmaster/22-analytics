//! 기여 연산 잡용 쿼리 (ARCHITECTURE §7).
//!
//! - 주문: `orders FINAL`의 현재 상태 (WP1이 쓰는 테이블, `version` 단조 증가)
//! - 식별자 확장: `identity_links` (member_hash → visitor_id들)
//! - 터치포인트: `touchpoints` 원본 값 (채널은 잡이 현재 규칙으로 다시 계산한다)
//! - 결과: `attributed_revenue` (ReplacingMergeTree(computed_at, is_deleted))
//!
//! 시각은 모두 UTC epoch 밀리초(`i64`)로 주고받는다. `DateTime64(3)`의 RowBinary 표현과 같다.

use crate::ProjectScope;
use clickhouse::{Client, Row};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 한 쿼리에 넣는 IN 목록 최대 길이 (max_query_size 256KiB 안쪽).
pub const IN_CHUNK: usize = 1_000;

/// 기여 연산 입력이 되는 주문 현재 상태.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct OrderForAttribution {
    pub order_id: String,
    pub status: String,
    #[serde(with = "clickhouse::serde::uuid::option")]
    pub visitor_id: Option<Uuid>,
    pub member_hash: String,
    pub effective_amount: i64,
    pub ordered_at_ms: i64,
    pub version: u64,
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

/// `attributed_revenue` 행 (insert용). 시각은 epoch ms.
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

/// 키(model, lookback_days, order_id, touch_index, ordered_at)별 가장 최근 행.
/// 삭제 표시된 행도 포함한다 (`computed_at`을 단조 증가시키기 위해).
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct ExistingRow {
    pub order_id: String,
    pub model: String,
    pub lookback_days: u8,
    pub touch_index: u16,
    pub ordered_at: i64,
    pub channel: String,
    pub utm_campaign: String,
    pub touch_ts: Option<i64>,
    pub amount: i64,
    pub is_assist: u8,
    pub is_deleted: u8,
    pub computed_at: i64,
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

/// `version > since_version`인 주문 (version 오름차순).
pub async fn changed_orders(
    _client: &Client,
    _scope: ProjectScope,
    _since_version: u64,
) -> anyhow::Result<Vec<OrderForAttribution>> {
    anyhow::bail!("미구현")
}

/// `ordered_at >= from_ms`인 주문 (ordered_at 오름차순).
pub async fn orders_since(
    _client: &Client,
    _scope: ProjectScope,
    _from_ms: i64,
) -> anyhow::Result<Vec<OrderForAttribution>> {
    anyhow::bail!("미구현")
}

/// 주문 ID로 조회한다 (없는 ID는 무시).
pub async fn orders_by_ids(
    _client: &Client,
    _scope: ProjectScope,
    _order_ids: &[String],
) -> anyhow::Result<Vec<OrderForAttribution>> {
    anyhow::bail!("미구현")
}

/// 회원 해시들에 연결된 visitor_id. 빈 해시는 무시한다.
pub async fn identity_links(
    _client: &Client,
    _scope: ProjectScope,
    _member_hashes: &[String],
) -> anyhow::Result<Vec<IdentityLink>> {
    anyhow::bail!("미구현")
}

/// visitor_id 집합의 `[from_ms, to_ms]` 터치포인트. (ts, session_id) 순.
pub async fn touchpoints(
    _client: &Client,
    _scope: ProjectScope,
    _visitor_ids: &[Uuid],
    _from_ms: i64,
    _to_ms: i64,
) -> anyhow::Result<Vec<TouchRow>> {
    anyhow::bail!("미구현")
}

/// 주문들의 기존 기여 결과 (키별 최신 행, 삭제 표시 포함).
pub async fn latest_rows(
    _client: &Client,
    _scope: ProjectScope,
    _order_ids: &[String],
) -> anyhow::Result<Vec<ExistingRow>> {
    anyhow::bail!("미구현")
}

/// 기여 결과를 한 번의 INSERT로 쓴다. 모든 행은 `scope` 프로젝트여야 한다.
pub async fn insert_rows(
    _client: &Client,
    _scope: ProjectScope,
    _rows: &[AttributedRow],
) -> anyhow::Result<()> {
    anyhow::bail!("미구현")
}

/// 매출 보존 검증 (§7.4): raw_sum 외 모델에서 Σamount ≠ effective_amount인 주문.
pub async fn conservation_check(
    _client: &Client,
    _scope: ProjectScope,
) -> anyhow::Result<Vec<BrokenOrder>> {
    anyhow::bail!("미구현")
}

/// 테스트 픽스처: 다른 패키지(WP1 주문 writer, 수집 MV)가 쓰는 테이블에 행을 직접 넣는다.
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
        pub status: String,
        #[serde(with = "clickhouse::serde::uuid::option")]
        pub visitor_id: Option<Uuid>,
        pub member_hash: String,
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
                status: "CONFIRMED".into(),
                visitor_id,
                member_hash: String::new(),
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
        pub click_id_type: String,
        pub referrer_host: String,
        pub page_host: String,
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
                click_id_type: String::new(),
                referrer_host: String::new(),
                page_host: "shop.example.com".into(),
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

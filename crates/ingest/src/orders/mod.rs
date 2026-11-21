//! S2S 주문 처리 (ARCHITECTURE §5).
//!
//! 주문 이벤트 1건 처리 순서 ([`OrderService::apply`]):
//! 1. 같은 주문의 처리를 직렬화한다 (order_id 해시로 고른 잠금).
//! 2. 주문의 기존 이벤트를 읽는다. 같은 멱등 키가 이미 있으면 append하지 않는다
//!    (내용이 다르면 아무것도 쓰지 않고 `conflict`를 돌려준다 → 409).
//!    클라이언트 이벤트는 주문당 [`MAX_CLIENT_EVENTS_PER_ORDER`]건까지만 받는다.
//! 3. 새 이벤트를 `order_events`에 append한다.
//! 4. 전체 이벤트로 상태를 계산하고([`project`]: fold + 이전 행의 `ordered_at` 유지), 현재 `orders`
//!    행과 다르면 새 version으로 insert한다. `ordered_at`은 처음 투영한 값으로 고정되므로 한 주문의
//!    모든 버전이 같은 월 파티션에 남는다 (ReplacingMergeTree는 파티션을 넘어 중복을 없애지 않는다).
//! 5. 쓴 뒤에는 이벤트를 다시 읽어 계산이 달라졌으면(다른 ingest 프로세스가 그 사이 append) 다시 쓴다.
//!    잠금은 프로세스 안에서만 유효하므로 이 재확인이 프로세스 간 경합을 보정한다 (단일 writer 권장, §10).
//!
//! 3이나 4가 실패하면 에러를 돌려주고 호출자는 5xx로 응답한다. 고객사가 재시도하면
//! 2에서 중복으로 판정되지만 4는 다시 수행되므로, 반쯤 쓰인 상태가 남지 않는다.

pub mod http;
#[cfg(test)]
mod tests;

use analytics_ch::ProjectScope;
use analytics_ch::orders::{self as ch_orders, OrderRow};
use analytics_core::Event;
use analytics_core::orders::{OrderEvent, OrderSource, client_purchase_event, project};
use analytics_core::settings::TrackingSettings;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, Semaphore};

/// 같은 주문 처리를 직렬화하는 잠금 개수. 서로 다른 주문이 같은 잠금을 공유해도 정확성에는 문제없다.
const LOCK_STRIPES: usize = 1024;
const PROJECT_TTL: Duration = Duration::from_secs(60);
/// 클라이언트 purchase 투영을 동시에 돌리는 백그라운드 작업 수. 넘치면 버리고 지표로 센다.
pub const CLIENT_CONCURRENCY: usize = 64;
/// 주문 하나에 받는 클라이언트 이벤트 상한. 넘는 이벤트는 무시한다
/// (반영할 때마다 주문의 전체 이벤트를 다시 읽으므로 무한히 쌓이면 비용이 제곱으로 는다).
pub const MAX_CLIENT_EVENTS_PER_ORDER: usize = 50;
/// 투영을 쓴 뒤 이벤트를 다시 읽어 확인하는 최대 횟수 (다른 프로세스와의 경합 보정).
const MAX_RECHECKS: usize = 3;

/// 주문 처리에 필요한 프로젝트 정보.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCtx {
    pub project_id: u32,
    pub active: bool,
    pub currency: String,
    pub salt_hex: String,
    pub client_purchase_as_order: bool,
}

/// [`OrderService::apply`] 결과.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Applied {
    /// 같은 멱등 키 이벤트가 이미 있었다.
    pub duplicate: bool,
    /// `orders`에 새 행을 썼다.
    pub projected: bool,
    /// 같은 멱등 키 이벤트가 있는데 내용(금액·시각)이 다르다. 아무것도 쓰지 않았다.
    pub conflict: bool,
    /// 클라이언트 이벤트 상한을 넘어 무시했다. 아무것도 쓰지 않았다.
    pub capped: bool,
}

/// [`OrderService::client_purchases`] 결과.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientPurchases {
    pub applied: usize,
    /// 저장소 오류로 실패한 건 (나머지는 계속 처리한다)
    pub failed: usize,
    /// 주문당 상한을 넘어 무시한 건
    pub capped: usize,
}

/// 주문 처리 지표 (`/metrics`).
#[derive(Debug, Default)]
pub struct OrderMetrics {
    /// 동시 작업 한도를 넘어 버린 클라이언트 purchase 수
    pub client_dropped: AtomicU64,
    /// 투영에 실패한 클라이언트 purchase 수
    pub client_failed: AtomicU64,
    /// 주문당 상한을 넘어 무시한 클라이언트 purchase 수
    pub client_capped: AtomicU64,
}

pub struct OrderService {
    pg: PgPool,
    ch: analytics_ch::Client,
    locks: Vec<Mutex<()>>,
    projects: RwLock<HashMap<u32, (Instant, Option<ProjectCtx>)>>,
    client_permits: Arc<Semaphore>,
    pub metrics: OrderMetrics,
}

impl OrderService {
    pub fn new(pg: PgPool, ch: analytics_ch::Client) -> Self {
        Self::with_client_concurrency(pg, ch, CLIENT_CONCURRENCY)
    }

    /// 클라이언트 purchase 백그라운드 작업 한도를 지정한다.
    pub fn with_client_concurrency(pg: PgPool, ch: analytics_ch::Client, n: usize) -> Self {
        Self {
            pg,
            ch,
            locks: (0..LOCK_STRIPES).map(|_| Mutex::new(())).collect(),
            projects: RwLock::new(HashMap::new()),
            client_permits: Arc::new(Semaphore::new(n)),
            metrics: OrderMetrics::default(),
        }
    }

    pub fn pg(&self) -> &PgPool {
        &self.pg
    }

    /// 프로젝트 정보 (60초 메모리 캐시).
    pub async fn project(&self, project_id: u32) -> anyhow::Result<Option<ProjectCtx>> {
        if let Some((at, ctx)) = self.projects.read().await.get(&project_id)
            && at.elapsed() < PROJECT_TTL
        {
            return Ok(ctx.clone());
        }
        let row =
            sqlx::query("SELECT status, currency, salt, settings FROM projects WHERE id = $1")
                .bind(i32::try_from(project_id).unwrap_or(-1))
                .fetch_optional(&self.pg)
                .await?;
        let ctx = row.map(|r| {
            let settings: TrackingSettings =
                serde_json::from_value(r.get("settings")).unwrap_or_default();
            ProjectCtx {
                project_id,
                active: r.get::<String, _>("status") == "active",
                currency: r.get("currency"),
                salt_hex: r.get("salt"),
                client_purchase_as_order: settings.client_purchase_as_order,
            }
        });
        self.projects
            .write()
            .await
            .insert(project_id, (Instant::now(), ctx.clone()));
        Ok(ctx)
    }

    fn lock_for(&self, project_id: u32, order_id: &str) -> &Mutex<()> {
        let mut h = DefaultHasher::new();
        (project_id, order_id).hash(&mut h);
        &self.locks[(h.finish() as usize) % self.locks.len()]
    }

    /// 주문 이벤트 1건을 append하고 주문 상태를 다시 투영한다.
    pub async fn apply(&self, ev: OrderEvent) -> anyhow::Result<Applied> {
        let _guard = self.lock_for(ev.project_id, &ev.order_id).lock().await;
        let scope = ProjectScope::authorized(ev.project_id);
        let mut events = ch_orders::order_events(&self.ch, scope, &ev.order_id).await?;
        let same_key = events
            .iter()
            .find(|e| e.idempotency_key == ev.idempotency_key);
        if let Some(prev) = same_key
            && !prev.same_payload(&ev)
        {
            // 같은 키로 다른 내용: 조용히 버리지 않고 호출자에게 알린다 (409)
            return Ok(Applied {
                duplicate: true,
                conflict: true,
                ..Applied::default()
            });
        }
        let duplicate = same_key.is_some();
        if !duplicate
            && ev.source == OrderSource::Client
            && events
                .iter()
                .filter(|e| e.source == OrderSource::Client)
                .count()
                >= MAX_CLIENT_EVENTS_PER_ORDER
        {
            return Ok(Applied {
                capped: true,
                ..Applied::default()
            });
        }
        let order_id = ev.order_id.clone();
        if !duplicate {
            ch_orders::insert_order_event(&self.ch, &ev).await?;
            events.push(ev);
        }
        let current = ch_orders::current_order(&self.ch, scope, &order_id).await?;
        let projected = self
            .write_projection(scope, &order_id, events, current)
            .await?;
        Ok(Applied {
            duplicate,
            projected,
            ..Applied::default()
        })
    }

    /// 주문의 이벤트를 다시 읽어 투영이 다르면 다시 쓴다. 썼으면 `true`.
    pub async fn reproject(&self, scope: ProjectScope, order_id: &str) -> anyhow::Result<bool> {
        let _guard = self.lock_for(scope.project_id(), order_id).lock().await;
        let events = ch_orders::order_events(&self.ch, scope, order_id).await?;
        let current = ch_orders::current_order(&self.ch, scope, order_id).await?;
        self.write_projection(scope, order_id, events, current)
            .await
    }

    /// `events`로 계산한 상태가 `current`와 다르면 새 version으로 쓴다. 쓴 뒤에는 이벤트와 현재 행을
    /// 다시 읽어, 그 사이 다른 writer(다른 ingest 프로세스: 줄무늬 잠금은 프로세스 안에서만 유효)가
    /// 이벤트를 추가했으면 다시 쓴다 (최대 [`MAX_RECHECKS`]번). 하나라도 썼으면 `true`.
    async fn write_projection(
        &self,
        scope: ProjectScope,
        order_id: &str,
        mut events: Vec<OrderEvent>,
        mut current: Option<OrderRow>,
    ) -> anyhow::Result<bool> {
        let mut wrote = false;
        for attempt in 0..=MAX_RECHECKS {
            let prev = current.as_ref().and_then(OrderRow::to_state);
            // 이전 행의 ordered_at을 유지한다 (파티션 고정, core::orders::project)
            let Some(state) = project(&events, prev.as_ref()) else {
                return Ok(wrote);
            };
            if prev.as_ref() == Some(&state) {
                return Ok(wrote);
            }
            let version = ch_orders::next_version(
                current.as_ref().map(|c| c.version),
                analytics_core::timeutil::now_ms(),
            );
            ch_orders::insert_order(&self.ch, &OrderRow::from_state(&state, version)).await?;
            wrote = true;
            if attempt == MAX_RECHECKS {
                tracing::warn!(order_id, "주문 투영 재확인 한도 초과");
                break;
            }
            events = ch_orders::order_events(&self.ch, scope, order_id).await?;
            current = ch_orders::current_order(&self.ch, scope, order_id).await?;
        }
        Ok(wrote)
    }

    /// 수집 파이프라인의 `purchase` 이벤트를 클라이언트 주문으로 투영한다
    /// (`client_purchase_as_order`가 켜진 프로젝트만).
    /// 같은 order_id의 S2S 주문이 있으면 `fold`가 S2S만 보므로 상태는 바뀌지 않는다.
    /// 한 건이 실패해도 나머지는 계속 처리하고 실패 수를 지표로 남긴다.
    pub async fn client_purchases(&self, events: &[Event]) -> ClientPurchases {
        let mut r = ClientPurchases::default();
        for e in events
            .iter()
            .filter(|e| e.event_name == analytics_core::orders::PURCHASE_EVENT)
        {
            let p = match self.project(e.project_id).await {
                Ok(Some(p)) => p,
                Ok(None) => continue,
                Err(err) => {
                    tracing::error!(error = %err, "클라이언트 purchase: 프로젝트 조회 실패");
                    r.failed += 1;
                    continue;
                }
            };
            if !p.active || !p.client_purchase_as_order {
                continue;
            }
            let Some(oe) = client_purchase_event(&p.currency, e) else {
                continue;
            };
            match self.apply(oe).await {
                Ok(a) if a.capped => r.capped += 1,
                Ok(_) => r.applied += 1,
                Err(err) => {
                    tracing::error!(error = %err, "클라이언트 purchase 주문 투영 실패");
                    r.failed += 1;
                }
            }
        }
        self.metrics
            .client_failed
            .fetch_add(r.failed as u64, Relaxed);
        self.metrics
            .client_capped
            .fetch_add(r.capped as u64, Relaxed);
        r
    }

    /// 백그라운드에서 [`Self::client_purchases`]를 실행한다 (수집 응답을 늦추지 않기 위해).
    /// 동시 작업은 [`CLIENT_CONCURRENCY`]개까지이고, 넘치면 버리고 `client_dropped`로 센다
    /// (클라이언트 주문은 원래 "유실 가능" 기준이다, §5.3).
    /// `events`는 [`purchases_of`]로 고른 purchase 이벤트다 (다른 이벤트는 무시한다).
    pub fn spawn_client_purchases(self: &Arc<Self>, mut purchases: Vec<Event>) {
        purchases.retain(is_purchase);
        if purchases.is_empty() {
            return;
        }
        let Ok(permit) = self.client_permits.clone().try_acquire_owned() else {
            self.metrics
                .client_dropped
                .fetch_add(purchases.len() as u64, Relaxed);
            tracing::warn!(
                n = purchases.len(),
                "클라이언트 purchase 투영 작업이 가득 차 버림"
            );
            return;
        };
        let svc = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            svc.client_purchases(&purchases).await;
        });
    }
}

fn is_purchase(e: &Event) -> bool {
    e.event_name == analytics_core::orders::PURCHASE_EVENT
}

/// 배치에서 purchase 이벤트만 복사한다 (파이프라인에 배치를 넘기기 전에 한 번만 호출).
pub fn purchases_of(events: &[Event]) -> Vec<Event> {
    events.iter().filter(|e| is_purchase(e)).cloned().collect()
}

//! S2S 주문 처리 (ARCHITECTURE §5).
//!
//! 주문 이벤트 1건 처리 순서 ([`OrderService::apply`]):
//! 1. 같은 주문의 처리를 직렬화한다 (order_id 해시로 고른 잠금).
//! 2. 주문의 기존 이벤트를 읽는다. 같은 멱등 키가 이미 있으면 append하지 않는다.
//! 3. 새 이벤트를 `order_events`에 append한다.
//! 4. 전체 이벤트로 상태를 계산하고([`project`]: fold + 이전 행의 `ordered_at` 유지), 현재 `orders`
//!    행과 다르면 새 version으로 insert한다. `ordered_at`은 처음 투영한 값으로 고정되므로 한 주문의
//!    모든 버전이 같은 월 파티션에 남는다 (ReplacingMergeTree는 파티션을 넘어 중복을 없애지 않는다).
//!
//! 3이나 4가 실패하면 에러를 돌려주고 호출자는 5xx로 응답한다. 고객사가 재시도하면
//! 2에서 중복으로 판정되지만 4는 다시 수행되므로, 반쯤 쓰인 상태가 남지 않는다.

pub mod http;
#[cfg(test)]
mod tests;

use analytics_ch::ProjectScope;
use analytics_ch::orders::{self as ch_orders, OrderRow};
use analytics_core::Event;
use analytics_core::orders::{OrderEvent, client_purchase_event, project};
use analytics_core::settings::TrackingSettings;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

/// 같은 주문 처리를 직렬화하는 잠금 개수. 서로 다른 주문이 같은 잠금을 공유해도 정확성에는 문제없다.
const LOCK_STRIPES: usize = 1024;
const PROJECT_TTL: Duration = Duration::from_secs(60);

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    /// 같은 멱등 키 이벤트가 이미 있었다.
    pub duplicate: bool,
    /// `orders`에 새 행을 썼다.
    pub projected: bool,
    /// 같은 멱등 키 이벤트가 있는데 내용(금액·시각)이 다르다. 아무것도 쓰지 않았다.
    pub conflict: bool,
}

pub struct OrderService {
    pg: PgPool,
    ch: analytics_ch::Client,
    locks: Vec<Mutex<()>>,
    projects: RwLock<HashMap<u32, (Instant, Option<ProjectCtx>)>>,
}

impl OrderService {
    pub fn new(pg: PgPool, ch: analytics_ch::Client) -> Self {
        Self {
            pg,
            ch,
            locks: (0..LOCK_STRIPES).map(|_| Mutex::new(())).collect(),
            projects: RwLock::new(HashMap::new()),
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
                projected: false,
                conflict: true,
            });
        }
        let duplicate = same_key.is_some();
        let order_id = ev.order_id.clone();
        if !duplicate {
            ch_orders::insert_order_event(&self.ch, &ev).await?;
            events.push(ev);
        }
        let current = ch_orders::current_order(&self.ch, scope, &order_id).await?;
        let prev = current.as_ref().and_then(OrderRow::to_state);
        // 이전 행의 ordered_at을 유지한다 (파티션 고정, core::orders::project)
        let state = project(&events, prev.as_ref()).expect("이벤트가 최소 1건 있다");
        if prev.as_ref() == Some(&state) {
            return Ok(Applied {
                duplicate,
                projected: false,
                conflict: false,
            });
        }
        let version =
            ch_orders::next_version(current.map(|c| c.version), crate::internal::now_ms());
        ch_orders::insert_order(&self.ch, &OrderRow::from_state(&state, version)).await?;
        Ok(Applied {
            duplicate,
            projected: true,
            conflict: false,
        })
    }

    /// 수집 파이프라인의 `purchase` 이벤트를 클라이언트 주문으로 투영한다
    /// (`client_purchase_as_order`가 켜진 프로젝트만). 반영한 이벤트 수를 돌려준다.
    /// 같은 order_id의 S2S 주문이 있으면 `fold`가 S2S만 보므로 상태는 바뀌지 않는다.
    pub async fn client_purchases(&self, events: &[Event]) -> anyhow::Result<usize> {
        let mut n = 0;
        for e in events
            .iter()
            .filter(|e| e.event_name == analytics_core::orders::PURCHASE_EVENT)
        {
            let Some(p) = self.project(e.project_id).await? else {
                continue;
            };
            if !p.active || !p.client_purchase_as_order {
                continue;
            }
            if let Some(oe) = client_purchase_event(&p.currency, e) {
                self.apply(oe).await?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// 백그라운드에서 [`Self::client_purchases`]를 실행한다 (수집 응답을 늦추지 않기 위해).
    pub fn spawn_client_purchases(self: &std::sync::Arc<Self>, events: &[Event]) {
        let purchases: Vec<Event> = events
            .iter()
            .filter(|e| e.event_name == analytics_core::orders::PURCHASE_EVENT)
            .cloned()
            .collect();
        if purchases.is_empty() {
            return;
        }
        let svc = self.clone();
        tokio::spawn(async move {
            if let Err(e) = svc.client_purchases(&purchases).await {
                tracing::error!(error = %e, "클라이언트 purchase 주문 투영 실패");
            }
        });
    }
}

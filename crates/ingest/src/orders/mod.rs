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
//! 5. (`ORDERS_MULTI_WRITER=true`일 때만) 쓴 뒤에는 이벤트를 다시 읽어 계산이 달라졌으면(다른 ingest
//!    프로세스가 그 사이 append) 다시 쓴다. 잠금은 프로세스 안에서만 유효하므로 이 재확인이 프로세스 간
//!    경합을 보정한다. 기본은 단일 writer(§10)라 프로세스 안 잠금으로 충분하므로 다시 읽지 않는다.
//!
//! 일괄 API([`OrderService::apply_batch`])는 배치 전체를 `order_id IN (…)` 읽기 두 번(이벤트, 현재 상태)과
//! 테이블당 multi-row insert 한 번으로 처리한다. 항목은 입력 순서대로 메모리에서 위 1~4를 그대로
//! 흉내 내므로(같은 주문의 항목은 직렬), 결과는 한 건씩 순서대로 보낸 것과 같다.
//! insert는 `async_insert=1, wait_for_async_insert=1`로 보낸다: 응답 전에 저장이 끝나는 것은 같고,
//! 동시에 들어온 주문들이 서버에서 한 파트로 모인다.
//!
//! 3이나 4가 실패하면 에러를 돌려주고 호출자는 5xx로 응답한다. 고객사가 재시도하면
//! 2에서 중복으로 판정되지만 4는 다시 수행되므로, 반쯤 쓰인 상태가 남지 않는다.

pub mod http;
#[cfg(test)]
mod tests;

use analytics_ch::ProjectScope;
use analytics_ch::orders::{self as ch_orders, OrderRow};
use analytics_core::Event;
use analytics_core::cache::TtlCache;
use analytics_core::orders::{OrderEvent, OrderSource, OrderState, client_purchase_event, project};
use analytics_core::settings::TrackingSettings;
use analytics_core::timeutil::now_ms;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use tokio::sync::{Mutex, MutexGuard, Semaphore};

/// 같은 주문 처리를 직렬화하는 잠금 개수. 서로 다른 주문이 같은 잠금을 공유해도 정확성에는 문제없다.
const LOCK_STRIPES: usize = 1024;
const PROJECT_TTL_MS: i64 = 60_000;
const PROJECT_CACHE_MAX: usize = 10_000;
/// 클라이언트 purchase 투영을 동시에 돌리는 백그라운드 작업 수. 넘치면 버리고 지표로 센다.
pub const CLIENT_CONCURRENCY: usize = 64;
/// 주문 하나에 받는 클라이언트 이벤트 상한. 넘는 이벤트는 무시한다
/// (반영할 때마다 주문의 전체 이벤트를 다시 읽으므로 무한히 쌓이면 비용이 제곱으로 는다).
pub const MAX_CLIENT_EVENTS_PER_ORDER: usize = 50;
/// 투영을 쓴 뒤 이벤트를 다시 읽어 확인하는 최대 횟수 (다른 프로세스와의 경합 보정, 다중 writer만).
const MAX_RECHECKS: usize = 3;
/// 이 서비스의 ClickHouse 클라이언트 설정: 작은 insert를 서버에서 모아 파트 수를 줄인다.
/// 완료까지 기다리므로(`wait_for_async_insert`) 응답 뒤 바로 읽어도 보인다.
pub const ASYNC_INSERT_SETTINGS: [(&str, &str); 2] =
    [("async_insert", "1"), ("wait_for_async_insert", "1")];

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

/// 새 이벤트를 받을지 판정한 결과 (단건·일괄 공용).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admit {
    New,
    Duplicate,
    Conflict,
    Capped,
}

/// 주문의 기존 이벤트(`events`)를 보고 `ev`를 판정한다.
fn admit(events: &[OrderEvent], ev: &OrderEvent) -> Admit {
    match events
        .iter()
        .find(|e| e.idempotency_key == ev.idempotency_key)
    {
        // 같은 키로 다른 내용: 조용히 버리지 않고 호출자에게 알린다 (409)
        Some(prev) if !prev.same_payload(ev) => Admit::Conflict,
        Some(_) => Admit::Duplicate,
        None if ev.source == OrderSource::Client
            && events
                .iter()
                .filter(|e| e.source == OrderSource::Client)
                .count()
                >= MAX_CLIENT_EVENTS_PER_ORDER =>
        {
            Admit::Capped
        }
        None => Admit::New,
    }
}

impl Applied {
    const CONFLICT: Self = Self {
        duplicate: true,
        projected: false,
        conflict: true,
        capped: false,
    };
    const CAPPED: Self = Self {
        duplicate: false,
        projected: false,
        conflict: false,
        capped: true,
    };

    /// 아무것도 쓰지 않고 끝난 결과(충돌·상한)는 저장소 쓰기 실패와 무관하다.
    fn wrote_nothing(&self) -> bool {
        self.conflict || self.capped
    }
}

/// 일괄 처리 중인 주문 하나의 메모리 상태.
struct Work {
    events: Vec<OrderEvent>,
    /// 배치 전에 저장소에 있던 이벤트 수 (그 뒤는 이번에 append할 것)
    stored: usize,
    /// 배치 전 현재 행의 version
    base_version: Option<u64>,
    state: Option<OrderState>,
    changed: bool,
}

pub struct OrderService {
    pg: PgPool,
    ch: analytics_ch::Client,
    locks: Vec<Mutex<()>>,
    projects: std::sync::Mutex<TtlCache<u32, ProjectCtx>>,
    client_permits: Arc<Semaphore>,
    /// 다른 ingest 프로세스도 주문을 쓰는가 (`ORDERS_MULTI_WRITER`). 켜면 쓴 뒤 다시 읽어 확인한다.
    multi_writer: bool,
    pub metrics: OrderMetrics,
}

impl OrderService {
    pub fn new(pg: PgPool, ch: analytics_ch::Client) -> Self {
        Self::with_client_concurrency(pg, ch, CLIENT_CONCURRENCY)
    }

    /// 클라이언트 purchase 백그라운드 작업 한도를 지정한다.
    pub fn with_client_concurrency(pg: PgPool, ch: analytics_ch::Client, n: usize) -> Self {
        let ch = ASYNC_INSERT_SETTINGS
            .iter()
            .fold(ch, |c, (k, v)| c.with_setting(*k, *v));
        Self {
            pg,
            ch,
            multi_writer: false,
            locks: (0..LOCK_STRIPES).map(|_| Mutex::new(())).collect(),
            projects: std::sync::Mutex::new(TtlCache::new(PROJECT_TTL_MS, PROJECT_CACHE_MAX)),
            client_permits: Arc::new(Semaphore::new(n)),
            metrics: OrderMetrics::default(),
        }
    }

    /// 다중 writer 모드(쓴 뒤 재확인)를 켜거나 끈다. 기본은 단일 writer.
    pub fn with_multi_writer(mut self, on: bool) -> Self {
        self.multi_writer = on;
        self
    }

    pub fn pg(&self) -> &PgPool {
        &self.pg
    }

    /// 프로젝트 정보 (60초 메모리 캐시).
    pub async fn project(&self, project_id: u32) -> anyhow::Result<Option<ProjectCtx>> {
        if let Some(ctx) = crate::lock(&self.projects).get(&project_id, now_ms()) {
            return Ok(ctx);
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
        crate::lock(&self.projects).put(project_id, ctx.clone(), now_ms());
        Ok(ctx)
    }

    fn stripe(&self, project_id: u32, order_id: &str) -> usize {
        let mut h = DefaultHasher::new();
        (project_id, order_id).hash(&mut h);
        (h.finish() as usize) % self.locks.len()
    }

    fn lock_for(&self, project_id: u32, order_id: &str) -> &Mutex<()> {
        &self.locks[self.stripe(project_id, order_id)]
    }

    /// 여러 주문의 잠금을 줄무늬 번호 오름차순으로 잡는다
    /// (교착 방지: 여러 개를 잡는 곳은 모두 이 순서, 나머지는 한 개만 잡는다).
    async fn lock_many(&self, project_id: u32, order_ids: &[String]) -> Vec<MutexGuard<'_, ()>> {
        let mut stripes: Vec<usize> = order_ids
            .iter()
            .map(|id| self.stripe(project_id, id))
            .collect();
        stripes.sort_unstable();
        stripes.dedup();
        let mut guards = Vec::with_capacity(stripes.len());
        for i in stripes {
            guards.push(self.locks[i].lock().await);
        }
        guards
    }

    /// 주문 이벤트 1건을 append하고 주문 상태를 다시 투영한다.
    pub async fn apply(&self, ev: OrderEvent) -> anyhow::Result<Applied> {
        let _guard = self.lock_for(ev.project_id, &ev.order_id).lock().await;
        let scope = ProjectScope::authorized(ev.project_id);
        // 잠금을 잡고 있으므로 두 읽기를 동시에 보내도 된다 (왕복 시간 1번)
        let (mut events, current) = tokio::try_join!(
            ch_orders::order_events(&self.ch, scope, &ev.order_id),
            ch_orders::current_order(&self.ch, scope, &ev.order_id),
        )?;
        let duplicate = match admit(&events, &ev) {
            Admit::Conflict => return Ok(Applied::CONFLICT),
            Admit::Capped => return Ok(Applied::CAPPED),
            Admit::Duplicate => true,
            Admit::New => false,
        };
        let order_id = ev.order_id.clone();
        if !duplicate {
            ch_orders::insert_order_event(&self.ch, &ev).await?;
            events.push(ev);
        }
        let projected = self
            .write_projection(scope, &order_id, events, current)
            .await?;
        Ok(Applied {
            duplicate,
            projected,
            ..Applied::default()
        })
    }

    /// 한 프로젝트의 주문 이벤트 여러 건을 입력 순서대로 처리한다 (일괄 API).
    ///
    /// 읽기는 배치 전체에 `order_id IN (…)` 두 번(동시에), 쓰기는 테이블당 multi-row insert 한 번이다.
    /// 항목마다 [`Self::apply`]의 판정·투영을 메모리에서 순서대로 수행하므로 항목별 결과·최종 상태·
    /// 저장된 이벤트는 한 건씩 순서대로 `apply`한 것과 같다. 주문마다 마지막 상태만 한 version으로 쓴다.
    ///
    /// 읽기가 실패하면 `Err`. 쓰기가 실패하면 아무것도 쓰지 않고 끝난 항목(충돌·상한)만 `Ok`이고
    /// 나머지 항목은 `Err`다 (재전송하면 중복으로 판정한 뒤 투영을 마저 쓴다).
    pub async fn apply_batch(
        &self,
        scope: ProjectScope,
        evs: Vec<OrderEvent>,
    ) -> anyhow::Result<Vec<anyhow::Result<Applied>>> {
        anyhow::ensure!(
            evs.iter().all(|e| e.project_id == scope.project_id()),
            "일괄 주문은 한 프로젝트만 담을 수 있다"
        );
        if evs.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids: Vec<String> = evs.iter().map(|e| e.order_id.clone()).collect();
        ids.sort_unstable();
        ids.dedup();
        let _guards = self.lock_many(scope.project_id(), &ids).await;
        let (stored, current) = tokio::try_join!(
            ch_orders::order_events_many(&self.ch, scope, &ids),
            ch_orders::current_orders(&self.ch, scope, &ids),
        )?;
        let mut work: HashMap<String, Work> = ids
            .into_iter()
            .map(|id| {
                let w = Work {
                    events: Vec::new(),
                    stored: 0,
                    base_version: None,
                    state: None,
                    changed: false,
                };
                (id, w)
            })
            .collect();
        for e in stored {
            if let Some(w) = work.get_mut(&e.order_id) {
                w.events.push(e);
                w.stored += 1;
            }
        }
        for r in current {
            if let Some(w) = work.get_mut(&r.order_id) {
                w.state = r.to_state();
                w.base_version = Some(r.version);
            }
        }

        // 입력 순서대로 apply를 메모리에서 흉내 낸다
        let mut results = Vec::with_capacity(evs.len());
        for ev in evs {
            let w = work
                .get_mut(&ev.order_id)
                .expect("모든 order_id를 미리 넣었다");
            let duplicate = match admit(&w.events, &ev) {
                Admit::Conflict => {
                    results.push(Applied::CONFLICT);
                    continue;
                }
                Admit::Capped => {
                    results.push(Applied::CAPPED);
                    continue;
                }
                Admit::Duplicate => true,
                Admit::New => {
                    w.events.push(ev);
                    false
                }
            };
            // 이전 상태의 ordered_at을 유지한다 (파티션 고정, core::orders::project)
            let projected = match project(&w.events, w.state.as_ref()) {
                Some(next) if w.state.as_ref() != Some(&next) => {
                    w.state = Some(next);
                    w.changed = true;
                    true
                }
                _ => false,
            };
            results.push(Applied {
                duplicate,
                projected,
                ..Applied::default()
            });
        }

        let now = now_ms();
        let rows: Vec<OrderRow> = work
            .values()
            .filter(|w| w.changed)
            .filter_map(|w| {
                let version = ch_orders::next_version(w.base_version, now);
                Some(OrderRow::from_state(w.state.as_ref()?, version))
            })
            .collect();
        let new_events: Vec<&OrderEvent> =
            work.values().flat_map(|w| &w.events[w.stored..]).collect();
        let written = async {
            ch_orders::insert_order_events(&self.ch, &new_events).await?;
            ch_orders::insert_orders(&self.ch, &rows).await?;
            if self.multi_writer {
                for r in &rows {
                    self.recheck(scope, &r.order_id).await?;
                }
            }
            anyhow::Ok(())
        }
        .await;
        Ok(match written {
            Ok(()) => results.into_iter().map(Ok).collect(),
            Err(e) => results
                .into_iter()
                .map(|a| {
                    if a.wrote_nothing() {
                        Ok(a)
                    } else {
                        Err(anyhow::anyhow!("{e:#}"))
                    }
                })
                .collect(),
        })
    }

    /// 주문의 이벤트를 다시 읽어 투영이 다르면 다시 쓴다. 썼으면 `true`.
    pub async fn reproject(&self, scope: ProjectScope, order_id: &str) -> anyhow::Result<bool> {
        let _guard = self.lock_for(scope.project_id(), order_id).lock().await;
        self.recheck(scope, order_id).await
    }

    /// 저장소에서 다시 읽어 투영이 다르면 다시 쓴다 (호출자가 잠금을 잡고 있다).
    async fn recheck(&self, scope: ProjectScope, order_id: &str) -> anyhow::Result<bool> {
        let (events, current) = tokio::try_join!(
            ch_orders::order_events(&self.ch, scope, order_id),
            ch_orders::current_order(&self.ch, scope, order_id),
        )?;
        self.write_projection(scope, order_id, events, current)
            .await
    }

    /// `events`로 계산한 상태가 `current`와 다르면 새 version으로 쓴다. 다중 writer 모드면 쓴 뒤에
    /// 이벤트와 현재 행을 다시 읽어, 그 사이 다른 writer(다른 ingest 프로세스: 줄무늬 잠금은 프로세스
    /// 안에서만 유효)가 이벤트를 추가했으면 다시 쓴다 (최대 [`MAX_RECHECKS`]번). 단일 writer면 잠금이
    /// 같은 주문의 쓰기를 모두 직렬화하므로 다시 읽지 않는다. 하나라도 썼으면 `true`.
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
            if !self.multi_writer {
                break;
            }
            if attempt == MAX_RECHECKS {
                tracing::warn!(order_id, "주문 투영 재확인 한도 초과");
                break;
            }
            (events, current) = tokio::try_join!(
                ch_orders::order_events(&self.ch, scope, order_id),
                ch_orders::current_order(&self.ch, scope, order_id),
            )?;
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

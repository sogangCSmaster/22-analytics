//! 기여 연산 잡 (ARCHITECTURE §7).
//!
//! 주문마다: 식별자 확장 → lookback 안 터치포인트 → 현재 채널 규칙으로 재분류 →
//! lookback × 모델 배분 → 기존 결과와 비교해 바뀐 주문만 `attributed_revenue`에 쓴다.
//! 같은 입력으로 몇 번을 돌려도 결과가 같다 (멱등).

pub mod compute;
pub mod worker;

use analytics_ch::attribution::{
    self as ch_attr, AttributedRow, BrokenOrder, OrderForAttribution, TouchRow,
};
use analytics_ch::{Client, ProjectScope};
use analytics_core::attribution::DAY_MS;
use analytics_core::channel::ChannelRule;
use analytics_pg::PgPool;
use analytics_pg::attribution_settings::{self as settings, AttributionSettings};
use std::collections::HashMap;
use uuid::Uuid;

/// 증분 잡 watermark 이름 (`job_watermarks.job`).
pub const JOB_INCREMENTAL: &str = "attribution";
/// 전체 재연산 기본 기간 (일).
pub const DEFAULT_FULL_DAYS: u32 = 35;
/// 한 번에 처리하는 주문 수.
pub const BATCH_ORDERS: usize = 500;
/// 전체 재연산이 한 번에 읽는 주문 수 (키셋 페이지). 클라이언트 메모리는 페이지 크기로 묶인다.
pub const PAGE_ORDERS: usize = 2_000;

/// 페이지·배치 크기 (테스트에서 작게 줄여 페이지 경계를 확인한다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// 전체 재연산에서 한 번에 읽는 주문 수
    pub page_orders: usize,
    /// 한 번에 계산·쓰는 주문 수 (조회 3번 + 쓰기 1번 단위)
    pub batch_orders: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            page_orders: PAGE_ORDERS,
            batch_orders: BATCH_ORDERS,
        }
    }
}
/// 증분 조회 겹침 구간 (ms). watermark 직전에 늦게 커밋된 version을 놓치지 않기 위해
/// `version > watermark − OVERLAP` 로 다시 읽는다. 바뀐 게 없으면 쓰지 않으므로 비용이 작다.
pub const INCREMENTAL_OVERLAP_MS: u64 = 120_000;

/// 실행 범위.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// watermark 이후 version이 바뀐 주문.
    Incremental,
    /// `ordered_at`이 최근 N일인 주문 전체 (늦은 터치포인트·identity·채널 규칙 변경 반영).
    Full { since_days: u32 },
    /// 지정한 주문만 (보존 검증 실패 주문 재연산).
    Orders(Vec<String>),
}

/// 실행 결과 집계.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub projects: usize,
    /// 조회한 주문 수
    pub orders_seen: usize,
    /// 결과가 바뀌어 행을 쓴 주문 수
    pub orders_written: usize,
    /// 쓴 행 수 (삭제 표시 포함)
    pub rows_written: usize,
    /// 주문 조회 횟수 (전체 재연산은 키셋 페이지 수, 나머지는 1)
    pub pages: usize,
}

/// 프로젝트 하나 또는 활성 프로젝트 전체를 실행한다. `now_ms`는 computed_at과 기간 계산 기준.
///
/// 프로젝트 하나가 실패해도 나머지는 계속 돌리고, 끝난 뒤 첫 오류를 돌려준다.
pub async fn run(
    ch: &Client,
    pg: &PgPool,
    project: Option<u32>,
    mode: &Mode,
    now_ms: i64,
) -> anyhow::Result<Report> {
    let mut total = Report::default();
    let mut first_err = None;
    for p in target_projects(pg, project).await? {
        match run_project(ch, pg, p, mode, now_ms).await {
            Ok(r) => {
                total.projects += 1;
                total.orders_seen += r.orders_seen;
                total.orders_written += r.orders_written;
                total.rows_written += r.rows_written;
                total.pages += r.pages;
            }
            Err(e) => {
                tracing::error!(project_id = p, error = %format!("{e:#}"), "기여 연산 실패");
                first_err.get_or_insert(e.context(format!("project {p}")));
            }
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(total),
    }
}

async fn target_projects(pg: &PgPool, project: Option<u32>) -> anyhow::Result<Vec<u32>> {
    Ok(match project {
        Some(p) => vec![p],
        None => settings::active_project_ids(pg).await?,
    })
}

/// 프로젝트 하나를 실행한다. 증분 모드는 성공한 배치까지 watermark를 올린다.
pub async fn run_project(
    ch: &Client,
    pg: &PgPool,
    project_id: u32,
    mode: &Mode,
    now_ms: i64,
) -> anyhow::Result<Report> {
    run_project_with(ch, pg, project_id, mode, now_ms, Limits::default()).await
}

/// [`run_project`]에 페이지·배치 크기를 지정한다.
///
/// 전체 재연산은 주문을 한 번에 다 읽지 않는다: `order_id` 키셋 페이지(`page_orders`건)를 이어 읽으며
/// 페이지마다 `batch_orders`건씩 처리하므로 메모리는 페이지 크기로 묶인다 (350만 주문 → 540MB였다).
/// 증분·지정 주문은 바뀐 주문만이라 작으므로 한 번에 읽는다.
pub async fn run_project_with(
    ch: &Client,
    pg: &PgPool,
    project_id: u32,
    mode: &Mode,
    now_ms: i64,
    limits: Limits,
) -> anyhow::Result<Report> {
    anyhow::ensure!(
        limits.page_orders > 0 && limits.batch_orders > 0,
        "페이지·배치 크기는 1 이상"
    );
    let scope = ProjectScope::authorized(project_id);
    let cfg = settings::load(pg, project_id).await?;
    let rules = settings::channel_rules(pg, project_id).await?;
    let ctx = BatchCtx {
        ch,
        scope,
        rules: &rules,
        cfg: &cfg,
        now_ms,
        batch_orders: limits.batch_orders,
    };
    let mut report = Report {
        projects: 1,
        ..Default::default()
    };

    match mode {
        Mode::Incremental => {
            let watermark = settings::get_watermark(pg, JOB_INCREMENTAL, project_id)
                .await?
                .map(|w| w.max(0) as u64);
            let since = watermark
                .unwrap_or(0)
                .saturating_sub(INCREMENTAL_OVERLAP_MS);
            let orders = ch_attr::changed_orders(ch, scope, since).await?;
            report.pages = 1;
            let mut wm = watermark;
            for batch in orders.chunks(limits.batch_orders) {
                ctx.process(batch, &mut report).await?;
                // changed_orders는 version 오름차순: 이 배치까지는 반영이 끝났다.
                let batch_max = batch.iter().map(|o| o.version).max().unwrap_or(0);
                if wm.is_none_or(|w| batch_max > w) {
                    settings::set_watermark(pg, JOB_INCREMENTAL, project_id, batch_max as i64)
                        .await?;
                    wm = Some(batch_max);
                }
            }
        }
        Mode::Full { since_days } => {
            let from = now_ms - i64::from(*since_days) * DAY_MS;
            let mut after: Option<String> = None;
            loop {
                let page = ch_attr::orders_since_page(
                    ch,
                    scope,
                    from,
                    after.as_deref(),
                    limits.page_orders,
                )
                .await?;
                report.pages += 1;
                ctx.process_all(&page, &mut report).await?;
                if page.len() < limits.page_orders {
                    break;
                }
                after = page.last().map(|o| o.order_id.clone());
            }
        }
        Mode::Orders(ids) => {
            let orders = ch_attr::orders_by_ids(ch, scope, ids).await?;
            report.pages = 1;
            ctx.process_all(&orders, &mut report).await?;
        }
    }
    tracing::info!(
        project_id,
        ?mode,
        seen = report.orders_seen,
        written = report.orders_written,
        rows = report.rows_written,
        pages = report.pages,
        "기여 연산 완료"
    );
    Ok(report)
}

/// 배치 처리에 필요한 공통 입력.
struct BatchCtx<'a> {
    ch: &'a Client,
    scope: ProjectScope,
    rules: &'a [ChannelRule],
    cfg: &'a AttributionSettings,
    now_ms: i64,
    batch_orders: usize,
}

impl BatchCtx<'_> {
    async fn process_all(
        &self,
        orders: &[OrderForAttribution],
        report: &mut Report,
    ) -> anyhow::Result<()> {
        for batch in orders.chunks(self.batch_orders) {
            self.process(batch, report).await?;
        }
        Ok(())
    }

    async fn process(
        &self,
        batch: &[OrderForAttribution],
        report: &mut Report,
    ) -> anyhow::Result<()> {
        let (written, rows) =
            process_batch(self.ch, self.scope, batch, self.rules, self.cfg, self.now_ms).await?;
        report.orders_seen += batch.len();
        report.orders_written += written;
        report.rows_written += rows;
        Ok(())
    }
}

/// 주문 배치 하나: 조회 3번(identity, 터치포인트, 기존 결과) + 쓰기 1번.
/// (결과가 바뀐 주문 수, 쓴 행 수)
async fn process_batch(
    ch: &Client,
    scope: ProjectScope,
    orders: &[OrderForAttribution],
    rules: &[ChannelRule],
    cfg: &AttributionSettings,
    now_ms: i64,
) -> anyhow::Result<(usize, usize)> {
    // 1. 식별자 확장: visitor_id ∪ identity_links[member_hash]
    let hashes: Vec<String> = orders.iter().map(|o| o.member_hash.clone()).collect();
    let mut by_member: HashMap<String, Vec<Uuid>> = HashMap::new();
    for l in ch_attr::identity_links(ch, scope, &hashes).await? {
        by_member
            .entry(l.member_hash)
            .or_default()
            .push(l.visitor_id);
    }
    let ids_of = |o: &OrderForAttribution| -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = o.visitor_id.into_iter().collect();
        if !o.member_hash.is_empty()
            && let Some(vs) = by_member.get(&o.member_hash)
        {
            ids.extend(vs);
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    let order_ids: Vec<Vec<Uuid>> = orders.iter().map(ids_of).collect();

    // 2. 터치포인트: 배치 전체 기간 [min(ordered_at) − 최대 lookback, max(ordered_at)]
    let all_ids: Vec<Uuid> = order_ids.iter().flatten().copied().collect();
    let live = orders.iter().filter(|o| compute::is_live(o));
    let from = live.clone().map(|o| o.ordered_at_ms).min();
    let to = live.map(|o| o.ordered_at_ms).max();
    let mut touches_by_visitor: HashMap<Uuid, Vec<TouchRow>> = HashMap::new();
    if let (Some(from), Some(to)) = (from, to)
        && !all_ids.is_empty()
    {
        let from = from - i64::from(cfg.max_lookback_days()) * DAY_MS;
        for t in ch_attr::touchpoints(ch, scope, &all_ids, from, to).await? {
            touches_by_visitor.entry(t.visitor_id).or_default().push(t);
        }
    }

    // 3. 기존 결과
    let ids: Vec<String> = orders.iter().map(|o| o.order_id.clone()).collect();
    let mut existing: HashMap<String, Vec<AttributedRow>> = HashMap::new();
    for r in ch_attr::latest_rows(ch, scope, &ids).await? {
        existing.entry(r.order_id.clone()).or_default().push(r);
    }

    // 4. 계산·비교
    let mut writes = Vec::new();
    let mut written_orders = 0;
    for (o, vids) in orders.iter().zip(&order_ids) {
        let rows: Vec<TouchRow> = vids
            .iter()
            .filter_map(|v| touches_by_visitor.get(v))
            .flatten()
            .cloned()
            .collect();
        let touches = compute::select_touches(vids, &rows, rules, cfg);
        let desired = compute::desired_rows(scope.project_id(), o, &touches, cfg);
        let prev = existing.get(&o.order_id).map(Vec::as_slice).unwrap_or(&[]);
        let w = compute::plan_writes(desired, prev, now_ms);
        if !w.is_empty() {
            written_orders += 1;
            writes.extend(w);
        }
    }
    ch_attr::insert_rows(ch, scope, &writes).await?;
    Ok((written_orders, writes.len()))
}

/// 매시간 보존 검증이 보는 기간 (일). 기본 lookback(30일)과 늦게 도착한 환불을 덮는다.
pub const DEFAULT_CONSERVATION_DAYS: u32 = 40;

/// 보존 검증 범위.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConservationWindow {
    /// 모든 주문 (주간 워커, CLI)
    Full,
    /// `ordered_at`이 최근 `days`일 안인 주문만 (매시간 워커)
    Recent { days: u32, now_ms: i64 },
}

impl ConservationWindow {
    fn ordered_since_ms(self) -> Option<i64> {
        match self {
            Self::Full => None,
            Self::Recent { days, now_ms } => Some(now_ms - i64::from(days) * DAY_MS),
        }
    }
}

/// 매출 보존 검증 (§7.4), 전체 주문. (project_id, 깨진 주문) 목록.
pub async fn conservation_check(
    ch: &Client,
    pg: &PgPool,
    project: Option<u32>,
) -> anyhow::Result<Vec<(u32, BrokenOrder)>> {
    conservation_check_window(ch, pg, project, ConservationWindow::Full).await
}

/// 매출 보존 검증 (§7.4), 범위 지정.
pub async fn conservation_check_window(
    ch: &Client,
    pg: &PgPool,
    project: Option<u32>,
    window: ConservationWindow,
) -> anyhow::Result<Vec<(u32, BrokenOrder)>> {
    let since = window.ordered_since_ms();
    let mut out = Vec::new();
    for p in target_projects(pg, project).await? {
        let broken = ch_attr::conservation_check(ch, ProjectScope::authorized(p), since).await?;
        out.extend(broken.into_iter().map(|b| (p, b)));
    }
    Ok(out)
}

pub use analytics_core::timeutil::now_ms;

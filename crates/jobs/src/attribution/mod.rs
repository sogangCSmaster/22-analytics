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
    let scope = ProjectScope::authorized(project_id);
    let cfg = settings::load(pg, project_id).await?;
    let rules = settings::channel_rules(pg, project_id).await?;

    let watermark = match mode {
        Mode::Incremental => settings::get_watermark(pg, JOB_INCREMENTAL, project_id)
            .await?
            .map(|w| w.max(0) as u64),
        _ => None,
    };
    let orders = match mode {
        Mode::Incremental => {
            let since = watermark
                .unwrap_or(0)
                .saturating_sub(INCREMENTAL_OVERLAP_MS);
            ch_attr::changed_orders(ch, scope, since).await?
        }
        Mode::Full { since_days } => {
            let from = now_ms - i64::from(*since_days) * DAY_MS;
            ch_attr::orders_since(ch, scope, from).await?
        }
        Mode::Orders(ids) => ch_attr::orders_by_ids(ch, scope, ids).await?,
    };

    let mut report = Report {
        projects: 1,
        ..Default::default()
    };
    let mut wm = watermark;
    for batch in orders.chunks(BATCH_ORDERS) {
        let (written, rows) = process_batch(ch, scope, batch, &rules, &cfg, now_ms).await?;
        report.orders_seen += batch.len();
        report.orders_written += written;
        report.rows_written += rows;
        if *mode == Mode::Incremental {
            // changed_orders는 version 오름차순: 이 배치까지는 반영이 끝났다.
            let batch_max = batch.iter().map(|o| o.version).max().unwrap_or(0);
            if wm.is_none_or(|w| batch_max > w) {
                settings::set_watermark(pg, JOB_INCREMENTAL, project_id, batch_max as i64).await?;
                wm = Some(batch_max);
            }
        }
    }
    tracing::info!(
        project_id,
        ?mode,
        seen = report.orders_seen,
        written = report.orders_written,
        rows = report.rows_written,
        "기여 연산 완료"
    );
    Ok(report)
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

/// 매출 보존 검증 (§7.4). (project_id, 깨진 주문) 목록.
pub async fn conservation_check(
    ch: &Client,
    pg: &PgPool,
    project: Option<u32>,
) -> anyhow::Result<Vec<(u32, BrokenOrder)>> {
    let mut out = Vec::new();
    for p in target_projects(pg, project).await? {
        let broken = ch_attr::conservation_check(ch, ProjectScope::authorized(p)).await?;
        out.extend(broken.into_iter().map(|b| (p, b)));
    }
    Ok(out)
}

pub use analytics_core::timeutil::now_ms;

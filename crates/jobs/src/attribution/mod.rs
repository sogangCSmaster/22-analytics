//! 기여 연산 잡 (ARCHITECTURE §7).
//!
//! 주문마다: 식별자 확장 → lookback 안 터치포인트 → 현재 채널 규칙으로 재분류 →
//! lookback × 모델 배분 → 기존 결과와 비교해 바뀐 주문만 `attributed_revenue`에 쓴다.
//! 같은 입력으로 몇 번을 돌려도 결과가 같다 (멱등).

pub mod compute;
pub mod worker;

use analytics_ch::Client;
use analytics_ch::attribution::BrokenOrder;
use analytics_pg::PgPool;

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
pub async fn run(
    _ch: &Client,
    _pg: &PgPool,
    _project: Option<u32>,
    _mode: &Mode,
    _now_ms: i64,
) -> anyhow::Result<Report> {
    anyhow::bail!("미구현")
}

/// 프로젝트 하나를 실행한다. 증분 모드는 성공한 배치까지 watermark를 올린다.
pub async fn run_project(
    _ch: &Client,
    _pg: &PgPool,
    _project_id: u32,
    _mode: &Mode,
    _now_ms: i64,
) -> anyhow::Result<Report> {
    anyhow::bail!("미구현")
}

/// 매출 보존 검증 (§7.4). (project_id, 깨진 주문) 목록.
pub async fn conservation_check(
    _ch: &Client,
    _pg: &PgPool,
    _project: Option<u32>,
) -> anyhow::Result<Vec<(u32, BrokenOrder)>> {
    anyhow::bail!("미구현")
}

/// 현재 시각 (epoch ms).
pub fn now_ms() -> i64 {
    time_now_ms()
}

fn time_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

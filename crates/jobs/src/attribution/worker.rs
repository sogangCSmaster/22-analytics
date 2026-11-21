//! 상주 워커: 증분 기여 연산(기본 5분), 전체 재연산(매일 04:00 KST), 보존 검증(매시간).

use super::{DEFAULT_FULL_DAYS, Mode};
use analytics_ch::Client;
use analytics_pg::PgPool;
use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;

/// KST = UTC+9 (서머타임 없음).
pub const KST_OFFSET_MS: i64 = 9 * 60 * 60 * 1000;

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// 증분 실행 간격
    pub incremental_every: Duration,
    /// 보존 검증 간격
    pub conservation_every: Duration,
    /// 전체 재연산 시각 (KST 시)
    pub full_hour_kst: u32,
    pub full_days: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            incremental_every: Duration::from_secs(300),
            conservation_every: Duration::from_secs(3_600),
            full_hour_kst: 4,
            full_days: DEFAULT_FULL_DAYS,
        }
    }
}

impl WorkerConfig {
    /// `ATTRIBUTION_INTERVAL_SECS`(기본 300)를 읽는다.
    pub fn from_env() -> Self {
        let mut c = Self::default();
        if let Some(secs) = std::env::var("ATTRIBUTION_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|s| *s > 0)
        {
            c.incremental_every = Duration::from_secs(secs);
        }
        c
    }
}

/// `now_ms` 이후(초과) 처음 오는 KST `hour`시 정각 (epoch ms).
pub fn next_daily_run_ms(now_ms: i64, hour_kst: u32) -> i64 {
    let day = 24 * 60 * 60 * 1000;
    let target = i64::from(hour_kst % 24) * 60 * 60 * 1000;
    let local = now_ms + KST_OFFSET_MS;
    let today = local - local.rem_euclid(day) + target;
    let next = if today > local { today } else { today + day };
    next - KST_OFFSET_MS
}

fn instant_at(epoch_ms: i64) -> Instant {
    let wait = (epoch_ms - super::now_ms()).max(0) as u64;
    Instant::now() + Duration::from_millis(wait)
}

/// `shutdown`이 끝날 때까지 잡을 돈다. 시작 직후 증분 1회를 실행한다.
/// 개별 실행 실패는 로그만 남기고 다음 주기에 다시 시도한다.
///
/// 실행 중에 종료 신호가 오면 그 실행을 버리고 바로 끝낸다. 잡은 멱등이고
/// watermark는 쓰기 성공 뒤에만 올라가므로 다음 기동 때 같은 결과로 이어진다.
pub async fn run(
    ch: &Client,
    pg: &PgPool,
    cfg: &WorkerConfig,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let mut shutdown = std::pin::pin!(shutdown);
    let mut next_inc = Instant::now();
    let mut next_cons = Instant::now() + cfg.conservation_every;
    let mut next_full = instant_at(next_daily_run_ms(super::now_ms(), cfg.full_hour_kst));
    tracing::info!(
        incremental_secs = cfg.incremental_every.as_secs(),
        full_hour_kst = cfg.full_hour_kst,
        "기여 연산 워커 시작"
    );
    loop {
        let wake = next_inc.min(next_cons).min(next_full);
        tokio::select! {
            _ = &mut shutdown => break,
            _ = tokio::time::sleep_until(wake) => {}
        }
        let now = Instant::now();
        let work = async {
            if now >= next_full {
                let mode = Mode::Full {
                    since_days: cfg.full_days,
                };
                if let Err(e) = super::run(ch, pg, None, &mode, super::now_ms()).await {
                    tracing::error!(error = %format!("{e:#}"), "전체 재연산 실패");
                }
                next_full = instant_at(next_daily_run_ms(super::now_ms(), cfg.full_hour_kst));
            }
            if now >= next_inc {
                if let Err(e) = super::run(ch, pg, None, &Mode::Incremental, super::now_ms()).await
                {
                    tracing::error!(error = %format!("{e:#}"), "증분 기여 연산 실패");
                }
                next_inc = Instant::now() + cfg.incremental_every;
            }
            if now >= next_cons {
                conservation_round(ch, pg).await;
                next_cons = Instant::now() + cfg.conservation_every;
            }
        };
        tokio::select! {
            _ = &mut shutdown => break,
            _ = work => {}
        }
    }
    tracing::info!("기여 연산 워커 종료");
    Ok(())
}

/// 보존 검증 후 깨진 주문을 알리고(error 로그) 다시 연산한다.
async fn conservation_round(ch: &Client, pg: &PgPool) {
    let broken = match super::conservation_check(ch, pg, None).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "보존 검증 실패");
            return;
        }
    };
    let mut by_project: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for (p, b) in broken {
        tracing::error!(
            project_id = p,
            order_id = %b.order_id,
            model = %b.model,
            lookback_days = b.lookback_days,
            attributed = b.attributed,
            effective_amount = b.effective_amount,
            "매출 보존 법칙 위반"
        );
        by_project.entry(p).or_default().push(b.order_id);
    }
    for (p, mut ids) in by_project {
        ids.sort();
        ids.dedup();
        if let Err(e) = super::run_project(ch, pg, p, &Mode::Orders(ids), super::now_ms()).await {
            tracing::error!(project_id = p, error = %format!("{e:#}"), "보존 위반 주문 재연산 실패");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 60 * 60 * 1000;
    /// 2026-11-21 00:00:00 UTC = 2026-11-21 09:00 KST
    const MIDNIGHT_UTC: i64 = 1_795_219_200_000;

    #[test]
    fn next_0400_kst() {
        // 09:00 KST → 다음날 04:00 KST (= 당일 19:00 UTC)
        assert_eq!(next_daily_run_ms(MIDNIGHT_UTC, 4), MIDNIGHT_UTC + 19 * H);
        // 03:59:59.999 KST → 같은 날 04:00 KST
        let before = MIDNIGHT_UTC + 19 * H - 1 - 24 * H;
        assert_eq!(next_daily_run_ms(before, 4), before + 1);
        // 정확히 04:00 KST면 다음날
        let at = MIDNIGHT_UTC + 19 * H;
        assert_eq!(next_daily_run_ms(at, 4), at + 24 * H);
        // 결과는 항상 미래, 24시간 이내
        for k in 0..48 {
            let now = MIDNIGHT_UTC + k * H / 2 + 123;
            let next = next_daily_run_ms(now, 4);
            assert!(next > now && next - now <= 24 * H);
            assert_eq!((next + KST_OFFSET_MS).rem_euclid(24 * H), 4 * H);
        }
    }

    #[test]
    fn interval_env_default() {
        let c = WorkerConfig::default();
        assert_eq!(c.incremental_every, Duration::from_secs(300));
        assert_eq!(c.full_hour_kst, 4);
        assert_eq!(c.full_days, 35);
    }
}

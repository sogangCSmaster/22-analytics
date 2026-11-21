//! 상주 워커: 증분 기여 연산(기본 5분), 전체 재연산(매일 04:00 KST), 보존 검증(매시간).

use super::{DEFAULT_FULL_DAYS, Mode};
use analytics_ch::Client;
use analytics_pg::PgPool;
use std::future::Future;
use std::time::Duration;

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
pub fn next_daily_run_ms(_now_ms: i64, _hour_kst: u32) -> i64 {
    todo!()
}

/// `shutdown`이 끝날 때까지 잡을 돈다. 시작 직후 증분 1회를 실행한다.
/// 개별 실행 실패는 로그만 남기고 다음 주기에 다시 시도한다.
pub async fn run(
    _ch: &Client,
    _pg: &PgPool,
    _cfg: &WorkerConfig,
    _shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let _ = Mode::Incremental;
    anyhow::bail!("미구현")
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

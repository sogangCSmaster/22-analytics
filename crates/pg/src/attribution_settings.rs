//! 기여 연산 설정(`attribution_settings`)과 잡 진행 위치(`job_watermarks`).
//!
//! 설정 행이 없으면 기본값(lookback 7·30일, 반감기 168시간, 자연검색·추천 포함, D4)을 쓴다.

use analytics_core::channel::ChannelRule;
use sqlx::PgPool;

/// 기본 lookback (일).
pub const DEFAULT_LOOKBACK_DAYS: [u8; 2] = [7, 30];
/// 기본 time_decay 반감기 (시간) = 7일.
pub const DEFAULT_HALF_LIFE_HOURS: u32 = 168;

/// 프로젝트별 기여 연산 설정.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributionSettings {
    /// 오름차순, 중복 없음, 1~255. 비어 있지 않다.
    pub lookback_days: Vec<u8>,
    pub half_life_hours: u32,
    /// 자연검색·자연 소셜(organic_search, organic_social)을 기여 대상에 넣을지.
    pub include_organic: bool,
    /// 추천(referral)을 기여 대상에 넣을지.
    pub include_referral: bool,
}

impl Default for AttributionSettings {
    fn default() -> Self {
        Self {
            lookback_days: DEFAULT_LOOKBACK_DAYS.to_vec(),
            half_life_hours: DEFAULT_HALF_LIFE_HOURS,
            include_organic: true,
            include_referral: true,
        }
    }
}

impl AttributionSettings {
    /// 가장 긴 lookback (일).
    pub fn max_lookback_days(&self) -> u8 {
        self.lookback_days.iter().copied().max().unwrap_or(0)
    }
}

/// 프로젝트 기여 설정을 읽는다. 행이 없으면 기본값.
pub async fn load(_pool: &PgPool, _project_id: u32) -> anyhow::Result<AttributionSettings> {
    anyhow::bail!("미구현")
}

/// 현재 채널 규칙: 프로젝트 규칙 → 전역 규칙 순, 각 그룹 안에서는 priority 오름차순.
pub async fn channel_rules(_pool: &PgPool, _project_id: u32) -> anyhow::Result<Vec<ChannelRule>> {
    anyhow::bail!("미구현")
}

/// 기여 연산 대상 프로젝트 (status = 'active'), id 오름차순.
pub async fn active_project_ids(_pool: &PgPool) -> anyhow::Result<Vec<u32>> {
    anyhow::bail!("미구현")
}

/// 잡의 프로젝트별 진행 위치. 기록이 없으면 `None`.
pub async fn get_watermark(
    _pool: &PgPool,
    _job: &str,
    _project_id: u32,
) -> anyhow::Result<Option<i64>> {
    anyhow::bail!("미구현")
}

/// 진행 위치를 기록한다 (덮어쓰기).
pub async fn set_watermark(
    _pool: &PgPool,
    _job: &str,
    _project_id: u32,
    _watermark: i64,
) -> anyhow::Result<()> {
    anyhow::bail!("미구현")
}

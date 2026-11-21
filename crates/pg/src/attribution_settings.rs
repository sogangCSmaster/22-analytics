//! 기여 연산 설정(`attribution_settings`)과 잡 진행 위치(`job_watermarks`).
//!
//! 설정 행이 없으면 기본값(lookback 7·30일, 반감기 168시간, 자연검색·추천 포함, D4)을 쓴다.

use analytics_core::channel::{ChannelRule, RuleMatch};
use sqlx::{PgPool, Row};

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

    /// DB 값 정리: lookback은 1~255만 남기고 정렬·중복 제거(없으면 기본값), 반감기 0 이하는 기본값.
    fn sanitized(
        lookback_days: &[i16],
        half_life_hours: i32,
        organic: bool,
        referral: bool,
    ) -> Self {
        let mut days: Vec<u8> = lookback_days
            .iter()
            .filter_map(|d| u8::try_from(*d).ok())
            .filter(|d| *d > 0)
            .collect();
        days.sort_unstable();
        days.dedup();
        if days.is_empty() {
            days = DEFAULT_LOOKBACK_DAYS.to_vec();
        }
        Self {
            lookback_days: days,
            half_life_hours: u32::try_from(half_life_hours)
                .ok()
                .filter(|h| *h > 0)
                .unwrap_or(DEFAULT_HALF_LIFE_HOURS),
            include_organic: organic,
            include_referral: referral,
        }
    }
}

/// 프로젝트 기여 설정을 읽는다. 행이 없으면 기본값.
pub async fn load(pool: &PgPool, project_id: u32) -> anyhow::Result<AttributionSettings> {
    let row = sqlx::query(
        "SELECT lookback_days, half_life_hours, include_organic, include_referral \
         FROM attribution_settings WHERE project_id = $1",
    )
    .bind(project_id as i32)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        None => AttributionSettings::default(),
        Some(r) => AttributionSettings::sanitized(
            &r.get::<Vec<i16>, _>("lookback_days"),
            r.get("half_life_hours"),
            r.get("include_organic"),
            r.get("include_referral"),
        ),
    })
}

/// 현재 채널 규칙: 프로젝트 규칙 → 전역 규칙 순, 각 그룹 안에서는 priority 오름차순.
/// `match`를 해석할 수 없는 규칙은 건너뛴다.
pub async fn channel_rules(pool: &PgPool, project_id: u32) -> anyhow::Result<Vec<ChannelRule>> {
    let rows = sqlx::query(
        "SELECT channel, match FROM channel_rules \
         WHERE project_id = $1 OR project_id IS NULL \
         ORDER BY (project_id IS NULL), priority, id",
    )
    .bind(project_id as i32)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let when: RuleMatch = serde_json::from_value(r.get("match")).ok()?;
            Some(ChannelRule {
                channel: r.get("channel"),
                when,
            })
        })
        .collect())
}

/// 기여 연산 대상 프로젝트 (status = 'active'), id 오름차순.
pub async fn active_project_ids(pool: &PgPool) -> anyhow::Result<Vec<u32>> {
    let ids: Vec<i32> =
        sqlx::query_scalar("SELECT id FROM projects WHERE status = 'active' ORDER BY id")
            .fetch_all(pool)
            .await?;
    Ok(ids.into_iter().map(|i| i as u32).collect())
}

/// 잡의 프로젝트별 진행 위치. 기록이 없으면 `None`.
pub async fn get_watermark(
    pool: &PgPool,
    job: &str,
    project_id: u32,
) -> anyhow::Result<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT watermark FROM job_watermarks WHERE job = $1 AND project_id = $2",
    )
    .bind(job)
    .bind(project_id as i32)
    .fetch_optional(pool)
    .await?)
}

/// 진행 위치를 기록한다 (덮어쓰기).
pub async fn set_watermark(
    pool: &PgPool,
    job: &str,
    project_id: u32,
    watermark: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO job_watermarks (job, project_id, watermark, updated_at) \
         VALUES ($1, $2, $3, now()) \
         ON CONFLICT (job, project_id) DO UPDATE \
         SET watermark = EXCLUDED.watermark, updated_at = now()",
    )
    .bind(job)
    .bind(project_id as i32)
    .bind(watermark)
    .execute(pool)
    .await?;
    Ok(())
}

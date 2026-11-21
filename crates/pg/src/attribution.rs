//! 프로젝트 기여 설정 (`attribution_settings`).

use sqlx::{PgExecutor, PgPool};

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AttributionSettings {
    pub lookback_days: Vec<i16>,
    /// `analytics_core::attribution::Model`의 snake_case 이름
    pub default_model: String,
    pub half_life_hours: i32,
    pub include_organic: bool,
    pub include_referral: bool,
}

impl Default for AttributionSettings {
    /// 테이블 기본값과 같다 (`0001_init.sql`).
    fn default() -> Self {
        Self {
            lookback_days: vec![7, 30],
            default_model: "last_click".into(),
            half_life_hours: 168,
            include_organic: true,
            include_referral: true,
        }
    }
}

/// 읽기 전용: 행이 없으면 기본값을 돌려주고 쓰지 않는다 (리포트처럼 viewer도 부르는 읽기 경로용).
pub async fn get_or_default(pool: &PgPool, project_id: i32) -> anyhow::Result<AttributionSettings> {
    Ok(sqlx::query_as::<_, AttributionSettings>(
        "SELECT lookback_days, default_model, half_life_hours, include_organic, include_referral \
         FROM attribution_settings WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or_default())
}

pub async fn put<'e>(
    ex: impl PgExecutor<'e>,
    project_id: i32,
    s: &AttributionSettings,
) -> anyhow::Result<AttributionSettings> {
    Ok(sqlx::query_as::<_, AttributionSettings>(
        "INSERT INTO attribution_settings \
           (project_id, lookback_days, default_model, half_life_hours, include_organic, include_referral) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (project_id) DO UPDATE SET \
           lookback_days = EXCLUDED.lookback_days, default_model = EXCLUDED.default_model, \
           half_life_hours = EXCLUDED.half_life_hours, include_organic = EXCLUDED.include_organic, \
           include_referral = EXCLUDED.include_referral \
         RETURNING lookback_days, default_model, half_life_hours, include_organic, include_referral",
    )
    .bind(project_id)
    .bind(&s.lookback_days)
    .bind(&s.default_model)
    .bind(s.half_life_hours)
    .bind(s.include_organic)
    .bind(s.include_referral)
    .fetch_one(ex)
    .await?)
}

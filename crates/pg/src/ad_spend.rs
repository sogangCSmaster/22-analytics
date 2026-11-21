//! 광고비 (`ad_spend`). CSV 업로드로 채우고 ROAS 계산에 쓴다.

use sqlx::{Acquire, PgPool, Postgres};
use time::Date;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Spend {
    pub day: Date,
    pub channel: String,
    pub campaign: String,
    pub amount: i64,
}

/// 한 문장에 넣는 행 수 (bind 배열 크기 제한 회피).
const CHUNK: usize = 5_000;

/// (day, channel, campaign) 기준 upsert. 한 트랜잭션이라 전부 반영되거나 전부 실패한다.
/// 호출자는 같은 키가 중복되지 않게 넘겨야 한다 (Postgres는 한 문장에서 같은 행을 두 번 갱신하지 못한다).
pub async fn upsert<'c>(
    conn: impl Acquire<'c, Database = Postgres>,
    project_id: i32,
    rows: &[Spend],
) -> anyhow::Result<()> {
    // 호출자의 트랜잭션 안이면 savepoint가 된다.
    let mut tx = conn.begin().await?;
    for chunk in rows.chunks(CHUNK) {
        let days: Vec<Date> = chunk.iter().map(|r| r.day).collect();
        let channels: Vec<&str> = chunk.iter().map(|r| r.channel.as_str()).collect();
        let campaigns: Vec<&str> = chunk.iter().map(|r| r.campaign.as_str()).collect();
        let amounts: Vec<i64> = chunk.iter().map(|r| r.amount).collect();
        sqlx::query(
            "INSERT INTO ad_spend (project_id, day, channel, campaign, amount) \
             SELECT $1, * FROM UNNEST($2::date[], $3::text[], $4::text[], $5::bigint[]) \
             ON CONFLICT (project_id, day, channel, campaign) DO UPDATE SET amount = EXCLUDED.amount",
        )
        .bind(project_id)
        .bind(&days)
        .bind(&channels)
        .bind(&campaigns)
        .bind(&amounts)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// `from..=to` 기간 (날짜, 채널, 캠페인 순).
pub async fn list(
    pool: &PgPool,
    project_id: i32,
    from: Date,
    to: Date,
) -> anyhow::Result<Vec<Spend>> {
    Ok(sqlx::query_as::<_, Spend>(
        "SELECT day, channel, campaign, amount FROM ad_spend \
         WHERE project_id = $1 AND day BETWEEN $2 AND $3 ORDER BY day, channel, campaign",
    )
    .bind(project_id)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?)
}

/// `from..=to` 기간의 채널별 광고비 합계 (실제 매출 리포트의 ROAS용).
///
/// 채널은 소문자로 묶는다 (대소문자만 다른 옛 행도 한 채널). 합계가 BIGINT를 넘으면
/// 오류 대신 BIGINT 최댓값으로 포화시킨다 (리포트 전체가 500이 되지 않게).
pub async fn sum_by_channel(
    pool: &PgPool,
    project_id: i32,
    from: Date,
    to: Date,
) -> anyhow::Result<std::collections::HashMap<String, i64>> {
    // BIGINT의 sum은 NUMERIC이라 넘치지 않는다. BIGINT로 되돌리기 전에 상한을 씌운다.
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT lower(channel), LEAST(sum(amount), 9223372036854775807)::bigint FROM ad_spend \
         WHERE project_id = $1 AND day BETWEEN $2 AND $3 GROUP BY lower(channel)",
    )
    .bind(project_id)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

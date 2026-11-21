//! 채널별 유입. 채널은 세션 범위 값이라 세션의 모든 이벤트가 같은 채널을 갖는다.

use super::DateRange;
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::ChannelTrafficRow;
use clickhouse::Row;
use serde::Deserialize;

#[derive(Row, Deserialize)]
struct Raw {
    channel: String,
    sessions: u64,
    entrances: u64,
    visitors: u64,
    pageviews: u64,
}

/// 세션 수 내림차순 (같으면 페이지뷰 내림차순, 채널 이름 오름차순).
pub async fn channels(
    client: &Client,
    scope: ProjectScope,
    range: DateRange,
) -> anyhow::Result<Vec<ChannelTrafficRow>> {
    let rows: Vec<Raw> = client
        .query(
            "SELECT toString(channel)   AS channel,
                    uniqMerge(sessions) AS sessions,
                    sum(entrances)      AS entrances,
                    uniqMerge(visitors) AS visitors,
                    sum(pageviews)      AS pageviews
             FROM daily_stats
             WHERE project_id = ? AND day BETWEEN toDate(?) AND toDate(?)
             GROUP BY channel
             ORDER BY sessions DESC, pageviews DESC, channel ASC",
        )
        .bind(scope.project_id())
        .bind(range.from_ymd())
        .bind(range.to_ymd())
        .fetch_all()
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| ChannelTrafficRow {
            channel: r.channel,
            sessions: r.sessions,
            entrances: r.entrances,
            visitors: r.visitors,
            pageviews: r.pageviews,
        })
        .collect())
}

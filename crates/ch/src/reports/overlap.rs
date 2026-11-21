//! 중복 매체 조합 (주문별 채널 경로).

use super::DateRange;
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::OverlapReport;

pub async fn overlap_paths(
    _client: &Client,
    _scope: ProjectScope,
    _range: DateRange,
    _lookback_days: u8,
    _limit: u32,
) -> anyhow::Result<OverlapReport> {
    anyhow::bail!("미구현")
}

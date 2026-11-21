//! 최근 주문 목록.

use super::DateRange;
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::RecentOrdersReport;

pub async fn recent_orders(
    _client: &Client,
    _scope: ProjectScope,
    _range: DateRange,
    _lookback_days: u8,
    _limit: u32,
    _offset: u64,
) -> anyhow::Result<RecentOrdersReport> {
    anyhow::bail!("미구현")
}

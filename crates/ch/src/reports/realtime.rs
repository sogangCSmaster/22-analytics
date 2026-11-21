//! 실시간 (최근 30분).

use crate::{Client, ProjectScope};
use analytics_core::dto::reports::RealtimeReport;
use time::OffsetDateTime;

pub const REALTIME_TOP_PAGES: u32 = 10;

pub async fn realtime(
    _client: &Client,
    _scope: ProjectScope,
    _now: OffsetDateTime,
) -> anyhow::Result<RealtimeReport> {
    anyhow::bail!("미구현")
}

//! 채널별 유입.

use super::DateRange;
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::ChannelTrafficRow;

pub async fn channels(
    _client: &Client,
    _scope: ProjectScope,
    _range: DateRange,
) -> anyhow::Result<Vec<ChannelTrafficRow>> {
    anyhow::bail!("미구현")
}

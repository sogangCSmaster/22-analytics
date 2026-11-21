//! 페이지별 지표.

use super::DateRange;
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::PageRow;

pub async fn pages(
    _client: &Client,
    _scope: ProjectScope,
    _range: DateRange,
    _path_contains: Option<&str>,
    _limit: u32,
) -> anyhow::Result<Vec<PageRow>> {
    anyhow::bail!("미구현")
}

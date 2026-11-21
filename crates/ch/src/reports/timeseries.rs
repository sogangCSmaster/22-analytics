//! 일별 추이.

use super::DateRange;
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::TimeseriesReport;

pub async fn timeseries(
    _client: &Client,
    _scope: ProjectScope,
    _range: DateRange,
) -> anyhow::Result<TimeseriesReport> {
    anyhow::bail!("미구현")
}

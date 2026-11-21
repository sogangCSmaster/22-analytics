//! 개요: 방문자·세션·페이지뷰·진입·이벤트와 매출, 직전 같은 길이 기간 비교.

use super::DateRange;
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::OverviewReport;

pub async fn overview(
    _client: &Client,
    _scope: ProjectScope,
    _range: DateRange,
) -> anyhow::Result<OverviewReport> {
    anyhow::bail!("미구현")
}

//! 실제 매출 (중복 소거) 리포트.

use super::DateRange;
use crate::{Client, ProjectScope};
use analytics_core::attribution::Model;
use analytics_core::dto::reports::RevenueReport;
use std::collections::HashMap;

pub async fn revenue_report(
    _client: &Client,
    _scope: ProjectScope,
    _range: DateRange,
    _model: Model,
    _lookback_days: u8,
    _spend: &HashMap<String, i64>,
) -> anyhow::Result<RevenueReport> {
    anyhow::bail!("미구현")
}

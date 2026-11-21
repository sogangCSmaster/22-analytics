//! 전자상거래 퍼널.

use super::DateRange;
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::FunnelReport;

pub const FUNNEL_STEPS: [&str; 4] = ["view_item", "add_to_cart", "begin_checkout", "purchase"];

pub async fn ecommerce_funnel(
    _client: &Client,
    _scope: ProjectScope,
    _range: DateRange,
) -> anyhow::Result<FunnelReport> {
    anyhow::bail!("미구현")
}

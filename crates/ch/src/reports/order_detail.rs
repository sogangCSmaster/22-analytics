//! 주문 상세.

use crate::{Client, ProjectScope};
use analytics_core::dto::reports::OrderDetail;

pub async fn order_detail(
    _client: &Client,
    _scope: ProjectScope,
    _order_id: &str,
    _lookback_days: u8,
) -> anyhow::Result<Option<OrderDetail>> {
    anyhow::bail!("미구현")
}

//! 이벤트 이름별 집계와 속성 값 분포.

use super::DateRange;
use crate::{Client, ProjectScope};
use analytics_core::dto::reports::{EventCountRow, PropertyValueRow};

pub async fn events(
    _client: &Client,
    _scope: ProjectScope,
    _range: DateRange,
    _limit: u32,
) -> anyhow::Result<Vec<EventCountRow>> {
    anyhow::bail!("미구현")
}

pub async fn event_properties(
    _client: &Client,
    _scope: ProjectScope,
    _range: DateRange,
    _event_name: &str,
    _prop_key: &str,
    _limit: u32,
) -> anyhow::Result<Vec<PropertyValueRow>> {
    anyhow::bail!("미구현")
}

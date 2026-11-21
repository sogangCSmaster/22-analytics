//! S2S 주문 처리 (ARCHITECTURE §5).

pub mod http;
#[cfg(test)]
mod tests;

use analytics_core::Event;
use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCtx {
    pub project_id: u32,
    pub active: bool,
    pub currency: String,
    pub salt_hex: String,
    pub client_purchase_as_order: bool,
}

pub struct OrderService {
    #[allow(dead_code)]
    pg: PgPool,
    #[allow(dead_code)]
    ch: analytics_ch::Client,
}

impl OrderService {
    pub fn new(pg: PgPool, ch: analytics_ch::Client) -> Self {
        Self { pg, ch }
    }

    pub async fn project(&self, project_id: u32) -> anyhow::Result<Option<ProjectCtx>> {
        let _ = project_id;
        todo!()
    }

    pub async fn client_purchases(&self, events: &[Event]) -> anyhow::Result<usize> {
        let _ = events;
        todo!()
    }
}

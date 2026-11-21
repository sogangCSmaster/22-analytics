//! 주문 API HTTP 라우트.

use super::OrderService;
use axum::Router;
use std::sync::Arc;

#[derive(Clone)]
pub struct OrdersHttp {
    pub svc: Arc<OrderService>,
    pub edge_secret: Arc<Vec<u8>>,
    pub emulation: bool,
}

pub fn router<S: Clone + Send + Sync + 'static>(st: OrdersHttp) -> Router<S> {
    let _ = st;
    Router::new()
}

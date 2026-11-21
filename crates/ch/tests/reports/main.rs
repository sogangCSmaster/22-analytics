//! `analytics_ch::reports` 통합 테스트. 테스트마다 격리된 ClickHouse DB를 쓴다 (`make up` 필요).
//!
//! 이벤트는 `rows::insert_events`로 넣어 MV(`daily_stats`, `touchpoints`)가 실제로 채워지게 하고,
//! `orders`·`attributed_revenue`는 WP1·WP2 잡이 쓰는 스키마 계약대로 직접 넣는다.

mod behavior;
mod common;
mod revenue;
mod traffic;

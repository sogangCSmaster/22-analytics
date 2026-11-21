//! 리포트 DTO.
//!
//! `analytics-ch::reports`가 만들고 api가 JSON으로 내보내며 대시보드(wasm)가 읽는다.
//! - 날짜: `YYYY-MM-DD` (Asia/Seoul 기준 일자)
//! - 시각: `YYYY-MM-DDTHH:MM:SS+09:00` (Asia/Seoul)
//! - 금액: `i64` 원(KRW)
//! - 비율: `f64` (0.25 = 25%). 분모가 0이면 0.0
//! - 정의되지 않는 값(광고비 0원인 ROAS 등)은 `Option`의 `None`

use crate::attribution::Model;
use serde::{Deserialize, Serialize};

/// 조회 기간 (양 끝 포함).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRange {
    pub from: String,
    pub to: String,
}

// ---------------------------------------------------------------- 개요

/// 개요 화면 지표 묶음.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverviewMetrics {
    /// 순 방문자 (근사 uniq)
    pub visitors: u64,
    /// 세션 수 (근사 uniq)
    pub sessions: u64,
    pub pageviews: u64,
    /// 세션 시작 이벤트 수
    pub entrances: u64,
    /// 전체 이벤트 수
    pub events: u64,
    /// 취소되지 않은 주문의 Σ effective_amount
    pub revenue: i64,
    /// 취소되지 않은 주문 수
    pub orders: u64,
}

/// 개요: 선택 기간과 바로 앞의 같은 길이 기간 (증감 계산용).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverviewReport {
    pub range: ReportRange,
    pub previous_range: ReportRange,
    pub current: OverviewMetrics,
    pub previous: OverviewMetrics,
}

// ---------------------------------------------------------------- 추이

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeseriesPoint {
    pub date: String,
    pub visitors: u64,
    pub sessions: u64,
    pub pageviews: u64,
}

/// 일별 추이. 데이터가 없는 날도 0으로 채워 기간의 모든 날짜가 들어 있다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeseriesReport {
    pub range: ReportRange,
    pub points: Vec<TimeseriesPoint>,
}

// ---------------------------------------------------------------- 유입·페이지·이벤트

/// 채널별 유입. 채널은 세션 범위 값이다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelTrafficRow {
    pub channel: String,
    pub sessions: u64,
    pub entrances: u64,
    pub visitors: u64,
    pub pageviews: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRow {
    pub path: String,
    pub pageviews: u64,
    /// 이 경로에서 이벤트를 남긴 순 방문자
    pub visitors: u64,
    /// 이 경로로 시작한 세션 수
    pub entrances: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventCountRow {
    pub event_name: String,
    pub count: u64,
    pub visitors: u64,
}

/// 이벤트 속성 값 분포.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyValueRow {
    pub value: String,
    pub count: u64,
    pub visitors: u64,
}

// ---------------------------------------------------------------- 실시간

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeMinute {
    /// `HH:MM` (Asia/Seoul), 구간 시작 분
    pub minute: String,
    pub pageviews: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimePage {
    pub path: String,
    pub pageviews: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeReport {
    /// 기준 시각
    pub now: String,
    /// 최근 5분 활성 방문자
    pub active_5m: u64,
    /// 최근 30분 활성 방문자
    pub active_30m: u64,
    /// 최근 30분 분당 페이지뷰. 항상 30개, 오래된 순, 마지막이 현재 분.
    pub per_minute: Vec<RealtimeMinute>,
    /// 최근 30분 상위 페이지
    pub top_pages: Vec<RealtimePage>,
}

// ---------------------------------------------------------------- 전자상거래 퍼널

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunnelStep {
    pub event_name: String,
    /// 이전 단계를 순서대로 거쳐 이 단계에 도달한 순 방문자
    pub visitors: u64,
    /// 이전 단계 대비 전환율 (첫 단계는 1.0, 이전 단계가 0명이면 0.0)
    pub rate_from_prev: f64,
    /// 첫 단계 대비 전환율
    pub rate_from_first: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunnelReport {
    pub range: ReportRange,
    pub steps: Vec<FunnelStep>,
}

// ---------------------------------------------------------------- 실제 매출 (중복 소거)

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelRevenueRow {
    pub channel: String,
    /// 매체가 자기 성과로 주장하는 매출 (raw_sum)
    pub raw_claimed: i64,
    /// 선택한 모델 기준 귀속 매출
    pub attributed: i64,
    /// 실제 매출 대비 귀속 비중
    pub share: f64,
    /// last_click 기준 어시스트 접점 수
    pub assists: u64,
    /// 광고비 (없으면 0)
    pub spend: i64,
    /// 귀속 매출 / 광고비. 광고비가 0이면 None
    pub roas: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RevenueReport {
    pub range: ReportRange,
    pub model: Model,
    pub lookback_days: u8,
    /// 매체 합산 매출 (Σ raw_sum)
    pub raw_sum: i64,
    /// 중복 소거 실매출 (선택 모델의 Σ amount)
    pub deduplicated: i64,
    /// raw_sum − deduplicated (0 미만이면 0)
    pub duplicate_amount: i64,
    /// duplicate_amount / raw_sum
    pub duplicate_rate: f64,
    /// 기여 연산된 주문 수
    pub orders: u64,
    pub spend_total: i64,
    /// deduplicated / spend_total. 광고비가 0이면 None
    pub roas: Option<f64>,
    /// 귀속 매출 내림차순
    pub channels: Vec<ChannelRevenueRow>,
}

/// 주문의 채널 경로 (연속 중복 채널은 하나로 합침).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlapPath {
    pub channels: Vec<String>,
    pub orders: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlapReport {
    pub range: ReportRange,
    pub lookback_days: u8,
    /// 서로 다른 채널을 2개 이상 거친 주문 수 (상위 N 제한 전 전체)
    pub multi_channel_orders: u64,
    /// 주문 수 내림차순
    pub paths: Vec<OverlapPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderInfo {
    pub order_id: String,
    pub source: String,
    pub status: String,
    pub currency: String,
    pub gross_amount: i64,
    pub net_amount: i64,
    pub refunded_amount: i64,
    pub effective_amount: i64,
    pub ordered_at: String,
}

/// 기여 대상 접점. `touch_index`는 1부터 시간순.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderTouch {
    pub touch_index: u16,
    pub ts: String,
    pub channel: String,
    pub campaign: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRow {
    /// 0 = 미귀속
    pub touch_index: u16,
    pub channel: String,
    pub campaign: String,
    pub amount: i64,
    pub is_assist: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelShares {
    pub model: Model,
    pub rows: Vec<ShareRow>,
}

/// 주문 상세 (드로어): 접점 타임라인과 모델별 배분.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderDetail {
    pub order: OrderInfo,
    pub lookback_days: u8,
    pub touches: Vec<OrderTouch>,
    /// `Model::ALL` 순서. 기여 연산 결과가 없는 모델은 빠진다.
    pub shares: Vec<ModelShares>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentOrderRow {
    pub order_id: String,
    pub ordered_at: String,
    pub effective_amount: i64,
    pub status: String,
    /// last_click 기준 채널. 기여 연산 전이거나 취소된 주문은 None
    pub last_click_channel: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentOrdersReport {
    pub range: ReportRange,
    /// 기간 안 전체 주문 수 (페이지네이션용)
    pub total: u64,
    /// 주문 시각 내림차순
    pub rows: Vec<RecentOrderRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revenue_report_json_contract() {
        let r = RevenueReport {
            range: ReportRange {
                from: "2026-11-20".into(),
                to: "2026-11-21".into(),
            },
            model: Model::LastClick,
            lookback_days: 7,
            raw_sum: 200_000,
            deduplicated: 100_000,
            duplicate_amount: 100_000,
            duplicate_rate: 0.5,
            orders: 1,
            spend_total: 0,
            roas: None,
            channels: vec![],
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["model"], "last_click");
        assert_eq!(v["range"]["from"], "2026-11-20");
        assert!(v["roas"].is_null());
        let back: RevenueReport = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }
}

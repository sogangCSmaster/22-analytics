//! 대시보드 리포트 쿼리 (docs/ARCHITECTURE.md §6.2, §7.3, §12.1).
//!
//! - 모든 함수는 [`ProjectScope`](crate::ProjectScope)를 받고 SQL에 `project_id = ?`를 넣는다.
//! - 사용자 입력(이벤트 이름, 속성 키, 경로 필터, 주문 번호)은 항상 `?` 바인딩으로 넘긴다.
//! - 일자는 1차 범위에서 Asia/Seoul 고정이다 (`daily_stats.day`와 같은 기준).
//! - 과거 트래픽 집계는 `daily_stats`, 실시간·퍼널·속성은 `events`, 매출은 `orders`·`attributed_revenue`를 읽는다.
//! - 순 방문자·세션은 ClickHouse `uniq` 근사값이다 (작은 집합에서는 정확).

mod channels;
mod events;
mod funnel;
mod order_detail;
mod overlap;
mod overview;
mod pages;
mod realtime;
mod recent_orders;
mod revenue;
mod timeseries;

pub use channels::channels;
pub use events::{event_properties, events};
pub use funnel::{FUNNEL_STEPS, ecommerce_funnel};
pub use order_detail::order_detail;
pub use overlap::overlap_paths;
pub use overview::overview;
pub use pages::pages;
pub use realtime::{REALTIME_TOP_PAGES, realtime};
pub use recent_orders::recent_orders;
pub use revenue::revenue_report;
pub use timeseries::timeseries;

use analytics_core::dto::reports::ReportRange;
use time::{Date, Duration, format_description::BorrowedFormatItem, macros::format_description};

/// 리포트 일자 기준 타임존 (1차 범위 고정).
pub const TZ: &str = "Asia/Seoul";
/// 한 번에 조회할 수 있는 최대 일수.
pub const MAX_RANGE_DAYS: u32 = 400;
/// 상위 N 목록의 최대 행 수.
pub const MAX_LIMIT: u32 = 1000;

const DATE_FMT: &[BorrowedFormatItem<'static>] = format_description!("[year]-[month]-[day]");

/// 기간 검증 오류.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RangeError {
    #[error("날짜 형식이 잘못되었습니다 (YYYY-MM-DD): {0}")]
    InvalidDate(String),
    #[error("시작일이 종료일보다 늦습니다")]
    Reversed,
    #[error("조회 기간은 최대 {MAX_RANGE_DAYS}일입니다 (요청 {0}일)")]
    TooLong(i64),
}

/// 조회 기간. `from`·`to` 모두 포함하는 Asia/Seoul 일자. 생성 시 검증된다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateRange {
    from: Date,
    to: Date,
}

impl DateRange {
    pub fn new(from: Date, to: Date) -> Result<Self, RangeError> {
        if from > to {
            return Err(RangeError::Reversed);
        }
        let days = (to - from).whole_days() + 1;
        if days > MAX_RANGE_DAYS as i64 {
            return Err(RangeError::TooLong(days));
        }
        Ok(Self { from, to })
    }

    /// `YYYY-MM-DD` 문자열 두 개로 만든다 (API 쿼리 파라미터용).
    pub fn parse(from: &str, to: &str) -> Result<Self, RangeError> {
        let p = |s: &str| {
            Date::parse(s, DATE_FMT)
                .map_err(|_| RangeError::InvalidDate(s.chars().take(32).collect()))
        };
        Self::new(p(from)?, p(to)?)
    }

    /// 하루짜리 기간.
    pub fn day(d: Date) -> Self {
        Self { from: d, to: d }
    }

    pub fn from(&self) -> Date {
        self.from
    }

    pub fn to(&self) -> Date {
        self.to
    }

    /// 기간 일수 (양 끝 포함).
    pub fn days(&self) -> u32 {
        ((self.to - self.from).whole_days() + 1) as u32
    }

    /// 바로 앞의 같은 길이 기간 (증감 비교용).
    pub fn previous(&self) -> Self {
        let len = Duration::days(self.days() as i64);
        Self {
            from: self.from - len,
            to: self.to - len,
        }
    }

    /// 기간 안의 날짜 (오름차순).
    pub fn dates(&self) -> impl Iterator<Item = Date> {
        let from = self.from;
        (0..self.days() as i64).map(move |i| from + Duration::days(i))
    }

    pub fn from_ymd(&self) -> String {
        fmt_date(self.from)
    }

    pub fn to_ymd(&self) -> String {
        fmt_date(self.to)
    }

    pub fn to_dto(&self) -> ReportRange {
        ReportRange {
            from: self.from_ymd(),
            to: self.to_ymd(),
        }
    }
}

pub(crate) fn fmt_date(d: Date) -> String {
    d.format(DATE_FMT).unwrap_or_default()
}

/// 상위 N 한도를 1..=MAX_LIMIT로 맞춘다.
pub(crate) fn clamp_limit(limit: u32) -> u32 {
    limit.clamp(1, MAX_LIMIT)
}

/// 0으로 나누면 0.0.
pub(crate) fn ratio(num: f64, den: f64) -> f64 {
    if den > 0.0 { num / den } else { 0.0 }
}

/// SQL 조각: Asia/Seoul 일자 시각 → `YYYY-MM-DDTHH:MM:SS+09:00`.
pub(crate) fn kst_iso(col: &str) -> String {
    format!("formatDateTime({col}, '%Y-%m-%dT%H:%i:%S+09:00', '{TZ}')")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn valid_range() {
        let r = DateRange::new(date!(2026 - 11 - 01), date!(2026 - 11 - 30)).unwrap();
        assert_eq!(r.days(), 30);
        assert_eq!(r.from_ymd(), "2026-11-01");
        assert_eq!(r.to_ymd(), "2026-11-30");
    }

    #[test]
    fn single_day_range() {
        let r = DateRange::new(date!(2026 - 11 - 20), date!(2026 - 11 - 20)).unwrap();
        assert_eq!(r.days(), 1);
        assert_eq!(r, DateRange::day(date!(2026 - 11 - 20)));
        assert_eq!(r.dates().collect::<Vec<_>>(), vec![date!(2026 - 11 - 20)]);
    }

    #[test]
    fn reversed_range_is_error() {
        assert_eq!(
            DateRange::new(date!(2026 - 11 - 21), date!(2026 - 11 - 20)),
            Err(RangeError::Reversed)
        );
    }

    #[test]
    fn range_length_limit() {
        // 400일은 허용, 401일은 거부
        let from = date!(2025 - 01 - 01);
        assert!(DateRange::new(from, from + Duration::days(399)).is_ok());
        assert_eq!(
            DateRange::new(from, from + Duration::days(400)),
            Err(RangeError::TooLong(401))
        );
    }

    #[test]
    fn parse_strings() {
        let r = DateRange::parse("2026-10-31", "2026-11-01").unwrap();
        assert_eq!(r.days(), 2);
        assert!(matches!(
            DateRange::parse("2026-13-01", "2026-11-01"),
            Err(RangeError::InvalidDate(_))
        ));
        assert!(matches!(
            DateRange::parse("'; DROP TABLE events --", "2026-11-01"),
            Err(RangeError::InvalidDate(_))
        ));
        assert_eq!(
            DateRange::parse("2026-11-02", "2026-11-01"),
            Err(RangeError::Reversed)
        );
    }

    #[test]
    fn previous_period_has_same_length() {
        let r = DateRange::parse("2026-11-20", "2026-11-21").unwrap();
        let p = r.previous();
        assert_eq!(
            (p.from_ymd(), p.to_ymd()),
            ("2026-11-18".into(), "2026-11-19".into())
        );
        // 월·연 경계
        let r = DateRange::parse("2026-01-01", "2026-01-31").unwrap();
        let p = r.previous();
        assert_eq!(
            (p.from_ymd(), p.to_ymd()),
            ("2025-12-01".into(), "2025-12-31".into())
        );
    }

    #[test]
    fn dates_cross_month() {
        let r = DateRange::parse("2026-10-30", "2026-11-02").unwrap();
        let ds: Vec<String> = r.dates().map(fmt_date).collect();
        assert_eq!(ds, ["2026-10-30", "2026-10-31", "2026-11-01", "2026-11-02"]);
    }

    #[test]
    fn limits_and_ratios() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(50), 50);
        assert_eq!(clamp_limit(1_000_000), MAX_LIMIT);
        assert_eq!(ratio(1.0, 0.0), 0.0);
        assert_eq!(ratio(1.0, 4.0), 0.25);
    }
}

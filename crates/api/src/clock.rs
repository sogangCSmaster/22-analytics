//! 시계 추상화. 리포트의 "오늘"(기본 기간·캐시 판단)과 실시간 기준 시각을 테스트에서 고정한다.

use time::{Date, OffsetDateTime, UtcOffset, macros::offset};

/// 리포트 일자 기준 타임존 (Asia/Seoul, 서머타임 없음).
pub const KST: UtcOffset = offset!(+9);

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> OffsetDateTime;

    /// Asia/Seoul 기준 오늘.
    fn today_kst(&self) -> Date {
        self.now().to_offset(KST).date()
    }
}

/// 실제 시계.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// 고정 시계 (테스트용).
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub OffsetDateTime);

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{date, datetime};

    #[test]
    fn today_is_seoul_date() {
        // UTC 15:00 = KST 다음 날 00:00
        assert_eq!(
            FixedClock(datetime!(2026-11-20 14:59:59 UTC)).today_kst(),
            date!(2026 - 11 - 20)
        );
        assert_eq!(
            FixedClock(datetime!(2026-11-20 15:00:00 UTC)).today_kst(),
            date!(2026 - 11 - 21)
        );
        // 다른 오프셋으로 들어와도 같은 결과
        assert_eq!(
            FixedClock(datetime!(2026-11-21 00:30:00 +9)).today_kst(),
            date!(2026 - 11 - 21)
        );
    }
}

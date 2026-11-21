//! 시각 변환 공용 함수. 저장·전송은 모두 unix ms(UTC)이고, 일자 기준은 1차 범위에서
//! Asia/Seoul(KST, UTC+9, 서머타임 없음) 고정이다.
//!
//! `now_ms()`만 시계를 읽는다. wasm(엣지)에서는 `SystemTime`이 없으므로 엣지는 자체 시계를 쓴다.

use time::{Date, Duration, OffsetDateTime, UtcOffset, macros::offset};

/// 리포트·롤업 일자 기준 타임존 (ClickHouse `toDate(ts, TZ_NAME)`과 같아야 한다).
pub const TZ_NAME: &str = "Asia/Seoul";
/// KST = UTC+9.
pub const KST: UtcOffset = offset!(+9);
/// KST 오프셋 (ms).
pub const KST_OFFSET_MS: i64 = 9 * 60 * 60 * 1000;

/// unix ms → `OffsetDateTime`(UTC). 표현 범위를 벗어나면 epoch.
pub fn ms_to_dt(ms: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// `OffsetDateTime` → unix ms (ms 미만은 버린다, 0 방향).
pub fn dt_to_ms(t: OffsetDateTime) -> i64 {
    i64::try_from(t.unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
}

/// unix ms가 속한 KST 일자.
pub fn kst_date(ms: i64) -> Date {
    (OffsetDateTime::UNIX_EPOCH + Duration::milliseconds(ms))
        .to_offset(KST)
        .date()
}

/// 현재 시각 (unix ms). wasm 타깃에는 없다.
#[cfg(not(target_arch = "wasm32"))]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{date, datetime};

    #[test]
    fn ms_round_trip() {
        for ms in [0, 1, 1_795_167_912_345, -1_000] {
            assert_eq!(dt_to_ms(ms_to_dt(ms)), ms);
        }
        assert_eq!(
            ms_to_dt(1_795_167_912_000),
            datetime!(2026-11-20 09:45:12 UTC)
        );
    }

    #[test]
    fn out_of_range_is_epoch() {
        assert_eq!(ms_to_dt(i64::MAX), OffsetDateTime::UNIX_EPOCH);
    }

    #[test]
    fn kst_date_boundary() {
        // 2026-11-20 14:59:59.999 UTC = 23:59:59.999 KST
        let midnight_kst = dt_to_ms(datetime!(2026-11-20 15:00 UTC));
        assert_eq!(kst_date(midnight_kst - 1), date!(2026 - 11 - 20));
        assert_eq!(kst_date(midnight_kst), date!(2026 - 11 - 21));
        assert_eq!(KST.whole_seconds() as i64 * 1000, KST_OFFSET_MS);
    }

    #[test]
    fn now_is_recent() {
        assert!(now_ms() > 1_700_000_000_000);
    }
}

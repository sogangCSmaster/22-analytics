//! 날짜·기간 계산 (Asia/Seoul 고정 +09:00, 서머타임 없음).
//!
//! 브라우저 시계(epoch ms)만 입력으로 받는 순수 함수다. 리포트 API의 `from`/`to`는
//! 양 끝을 포함하는 `YYYY-MM-DD`(서울 기준 일자)다.

use serde::{Deserialize, Serialize};

pub const SEOUL_OFFSET_MS: i64 = 9 * 3600 * 1000;
const DAY_MS: i64 = 86_400_000;
/// 직접 선택 기간 상한 (API 상한과 같다)
pub const MAX_RANGE_DAYS: i32 = 366;

/// 1970-01-01부터의 일 수.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Day(pub i32);

const WEEKDAYS: [&str; 7] = ["월", "화", "수", "목", "금", "토", "일"];

impl Day {
    pub fn from_ymd(y: i32, m: u32, d: u32) -> Option<Day> {
        todo!()
    }

    pub fn ymd(self) -> (i32, u32, u32) {
        todo!()
    }

    /// 엄격한 `YYYY-MM-DD`.
    pub fn parse(s: &str) -> Option<Day> {
        todo!()
    }

    pub fn iso(self) -> String {
        todo!()
    }

    pub fn add(self, n: i32) -> Day {
        Day(self.0 + n)
    }

    /// 서울 기준 오늘.
    pub fn seoul_today(now_ms: i64) -> Day {
        todo!()
    }

    /// 0 = 월요일
    pub fn weekday(self) -> usize {
        todo!()
    }

    /// 축 라벨: `11.21`
    pub fn short(self) -> String {
        todo!()
    }

    /// `11월 21일 (토)`
    pub fn long_kr(self) -> String {
        todo!()
    }
}

/// 상단 바 기간 선택.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    Today,
    #[default]
    Days7,
    Days30,
    Days90,
    Custom,
}

impl Preset {
    pub const ALL: [Preset; 5] = [
        Preset::Today,
        Preset::Days7,
        Preset::Days30,
        Preset::Days90,
        Preset::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Today => "오늘",
            Preset::Days7 => "7일",
            Preset::Days30 => "30일",
            Preset::Days90 => "90일",
            Preset::Custom => "직접 선택",
        }
    }
}

/// 양 끝 포함 기간.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DateRange {
    pub from: Day,
    pub to: Day,
}

impl DateRange {
    pub fn days(self) -> i32 {
        todo!()
    }

    /// 바로 앞의 같은 길이 기간.
    pub fn previous(self) -> DateRange {
        todo!()
    }

    pub fn each_day(self) -> Vec<Day> {
        todo!()
    }

    /// `2026.11.15 – 11.21`, 하루면 `2026.11.21`, 해가 바뀌면 둘 다 연도 표기.
    pub fn label(self) -> String {
        todo!()
    }
}

/// 프리셋 기간. 7일 = 오늘 포함 최근 7일.
pub fn preset_range(p: Preset, today: Day) -> Option<DateRange> {
    todo!()
}

/// 직접 선택 입력 검증.
pub fn custom_range(from: &str, to: &str, today: Day) -> Result<DateRange, &'static str> {
    todo!()
}

/// `2026-11-21T14:03:00+09:00` → `11월 21일 14:03`. 형식이 다르면 입력을 그대로 돌려준다.
pub fn datetime_kr(s: &str) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-11-21 00:30 KST == 2026-11-20 15:30 UTC
    const NOV21_0030_KST: i64 = 1_795_188_600_000;

    #[test]
    fn civil_roundtrip_and_leap_years() {
        assert_eq!(Day::from_ymd(1970, 1, 1), Some(Day(0)));
        let d = Day::from_ymd(2024, 2, 29).unwrap();
        assert_eq!(d.ymd(), (2024, 2, 29));
        assert_eq!(d.add(1).ymd(), (2024, 3, 1));
        assert_eq!(Day::from_ymd(2026, 2, 29), None);
        assert_eq!(Day::from_ymd(2100, 2, 29), None);
        assert!(Day::from_ymd(2000, 2, 29).is_some());
        assert_eq!(Day::from_ymd(2026, 13, 1), None);
        assert_eq!(Day::from_ymd(2026, 4, 31), None);
        for n in [-1000, -1, 0, 1, 20_000, 30_000] {
            let (y, m, dd) = Day(n).ymd();
            assert_eq!(Day::from_ymd(y, m, dd), Some(Day(n)));
        }
    }

    #[test]
    fn parse_is_strict() {
        assert_eq!(Day::parse("2026-11-21").map(Day::iso).as_deref(), Some("2026-11-21"));
        for bad in ["", "2026-1-21", "2026-11-21T00:00", "2026/11/21", "２０２６-11-21", "abcd-ef-gh", "2026-02-30"] {
            assert_eq!(Day::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn seoul_today_crosses_utc_midnight() {
        assert_eq!(Day::seoul_today(NOV21_0030_KST).iso(), "2026-11-21");
        // 1시간 전은 서울 기준 전날 23:30
        assert_eq!(Day::seoul_today(NOV21_0030_KST - 3_600_000).iso(), "2026-11-20");
        // 1970 이전 (음수)도 내림으로 계산
        assert_eq!(Day::seoul_today(-SEOUL_OFFSET_MS - 1).iso(), "1969-12-31");
    }

    #[test]
    fn weekday_and_labels() {
        let d = Day::parse("2026-11-21").unwrap();
        assert_eq!(d.weekday(), 5); // 토요일
        assert_eq!(d.short(), "11.21");
        assert_eq!(d.long_kr(), "11월 21일 (토)");
    }

    #[test]
    fn presets_include_today() {
        let today = Day::parse("2026-11-21").unwrap();
        let r = preset_range(Preset::Days7, today).unwrap();
        assert_eq!((r.from.iso(), r.to.iso()), ("2026-11-15".into(), "2026-11-21".into()));
        assert_eq!(r.days(), 7);
        assert_eq!(preset_range(Preset::Today, today).unwrap().days(), 1);
        assert_eq!(preset_range(Preset::Days90, today).unwrap().days(), 90);
        assert_eq!(preset_range(Preset::Custom, today), None);
    }

    #[test]
    fn previous_period_has_same_length() {
        let today = Day::parse("2026-03-01").unwrap();
        let r = preset_range(Preset::Days30, today).unwrap();
        let p = r.previous();
        assert_eq!(p.days(), 30);
        assert_eq!(p.to.add(1), r.from);
        let t = preset_range(Preset::Today, today).unwrap().previous();
        assert_eq!((t.from.iso(), t.to.iso()), ("2026-02-28".into(), "2026-02-28".into()));
    }

    #[test]
    fn each_day_lists_all() {
        let r = custom_range("2026-12-30", "2027-01-02", Day::parse("2027-01-05").unwrap()).unwrap();
        let v: Vec<String> = r.each_day().into_iter().map(Day::iso).collect();
        assert_eq!(v, ["2026-12-30", "2026-12-31", "2027-01-01", "2027-01-02"]);
    }

    #[test]
    fn custom_range_validation() {
        let today = Day::parse("2026-11-21").unwrap();
        assert!(custom_range("2026-11-01", "2026-11-21", today).is_ok());
        assert!(custom_range(" 2026-11-01 ", "2026-11-01", today).is_ok());
        assert_eq!(custom_range("2026-11-22", "2026-11-21", today).unwrap_err(), "시작일이 종료일보다 늦습니다");
        assert_eq!(custom_range("2026-11-01", "2026-11-22", today).unwrap_err(), "미래 날짜는 선택할 수 없습니다");
        assert_eq!(custom_range("x", "2026-11-21", today).unwrap_err(), "날짜 형식이 올바르지 않습니다");
        assert_eq!(custom_range("2025-01-01", "2026-11-21", today).unwrap_err(), "기간은 최대 366일까지 선택할 수 있습니다");
    }

    #[test]
    fn range_labels() {
        let today = Day::parse("2026-11-21").unwrap();
        assert_eq!(preset_range(Preset::Days7, today).unwrap().label(), "2026.11.15 – 11.21");
        assert_eq!(preset_range(Preset::Today, today).unwrap().label(), "2026.11.21");
        let r = custom_range("2025-12-30", "2026-01-02", today).unwrap();
        assert_eq!(r.label(), "2025.12.30 – 2026.01.02");
    }

    #[test]
    fn datetime_display() {
        assert_eq!(datetime_kr("2026-11-21T14:03:59+09:00"), "11월 21일 14:03");
        assert_eq!(datetime_kr("2026-01-05T09:00:00Z"), "1월 5일 09:00");
        assert_eq!(datetime_kr("나쁜값<script>"), "나쁜값<script>");
        assert_eq!(datetime_kr(""), "");
    }
}

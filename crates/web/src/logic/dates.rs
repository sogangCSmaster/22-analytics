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
        if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
            return None;
        }
        // Howard Hinnant, days_from_civil
        let y = if m <= 2 { y - 1 } else { y } as i64;
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let mp = (m as i64 + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        Some(Day((era * 146_097 + doe - 719_468) as i32))
    }

    pub fn ymd(self) -> (i32, u32, u32) {
        let z = self.0 as i64 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let y = (yoe + era * 400 + if m <= 2 { 1 } else { 0 }) as i32;
        (y, m, d)
    }

    /// 엄격한 `YYYY-MM-DD`.
    pub fn parse(s: &str) -> Option<Day> {
        let b = s.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let num = |r: std::ops::Range<usize>| -> Option<u32> {
            let part = &s[r];
            if part.bytes().all(|c| c.is_ascii_digit()) {
                part.parse().ok()
            } else {
                None
            }
        };
        Day::from_ymd(num(0..4)? as i32, num(5..7)?, num(8..10)?)
    }

    pub fn iso(self) -> String {
        let (y, m, d) = self.ymd();
        format!("{y:04}-{m:02}-{d:02}")
    }

    pub fn add(self, n: i32) -> Day {
        Day(self.0 + n)
    }

    /// 서울 기준 오늘.
    pub fn seoul_today(now_ms: i64) -> Day {
        Day((now_ms + SEOUL_OFFSET_MS).div_euclid(DAY_MS) as i32)
    }

    /// 0 = 월요일
    pub fn weekday(self) -> usize {
        // 1970-01-01은 목요일(3)
        (self.0 as i64 + 3).rem_euclid(7) as usize
    }

    /// 축 라벨: `11.21`
    pub fn short(self) -> String {
        let (_, m, d) = self.ymd();
        format!("{m}.{d:02}")
    }

    /// `11월 21일 (토)`
    pub fn long_kr(self) -> String {
        let (_, m, d) = self.ymd();
        format!("{m}월 {d}일 ({})", WEEKDAYS[self.weekday()])
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
        self.to.0 - self.from.0 + 1
    }

    /// 바로 앞의 같은 길이 기간.
    pub fn previous(self) -> DateRange {
        let to = self.from.add(-1);
        DateRange {
            from: to.add(-(self.days() - 1)),
            to,
        }
    }

    #[allow(dead_code)] // 빈 날 채우기용 (API가 이미 채워 준다)
    pub fn each_day(self) -> Vec<Day> {
        (self.from.0..=self.to.0).map(Day).collect()
    }

    /// `2026.11.15 – 11.21`, 하루면 `2026.11.21`, 해가 바뀌면 둘 다 연도 표기.
    pub fn label(self) -> String {
        let (fy, fm, fd) = self.from.ymd();
        let (ty, tm, td) = self.to.ymd();
        if self.from == self.to {
            format!("{fy}.{fm:02}.{fd:02}")
        } else if fy == ty {
            format!("{fy}.{fm:02}.{fd:02} – {tm:02}.{td:02}")
        } else {
            format!("{fy}.{fm:02}.{fd:02} – {ty}.{tm:02}.{td:02}")
        }
    }
}

/// 프리셋 기간. 7일 = 오늘 포함 최근 7일.
pub fn preset_range(p: Preset, today: Day) -> Option<DateRange> {
    let n = match p {
        Preset::Today => 1,
        Preset::Days7 => 7,
        Preset::Days30 => 30,
        Preset::Days90 => 90,
        Preset::Custom => return None,
    };
    Some(DateRange {
        from: today.add(-(n - 1)),
        to: today,
    })
}

/// 직접 선택 입력 검증.
pub fn custom_range(from: &str, to: &str, today: Day) -> Result<DateRange, &'static str> {
    let (Some(f), Some(t)) = (Day::parse(from.trim()), Day::parse(to.trim())) else {
        return Err("날짜 형식이 올바르지 않습니다");
    };
    if f > t {
        return Err("시작일이 종료일보다 늦습니다");
    }
    if t > today {
        return Err("미래 날짜는 선택할 수 없습니다");
    }
    let r = DateRange { from: f, to: t };
    if r.days() > MAX_RANGE_DAYS {
        return Err("기간은 최대 366일까지 선택할 수 있습니다");
    }
    Ok(r)
}

/// `2026-11-21T14:03:00+09:00` → `11월 21일 14:03`. 형식이 다르면 입력을 그대로 돌려준다.
pub fn datetime_kr(s: &str) -> String {
    let b = s.as_bytes();
    let ok = b.len() >= 16
        && s.is_char_boundary(10)
        && s.is_char_boundary(16)
        && b[10] == b'T'
        && b[13] == b':'
        && [11, 12, 14, 15].iter().all(|&i| b[i].is_ascii_digit());
    match ok.then(|| Day::parse(&s[..10])).flatten() {
        Some(d) => {
            let (_, m, dd) = d.ymd();
            format!("{m}월 {dd}일 {}", &s[11..16])
        }
        None => s.to_owned(),
    }
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
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

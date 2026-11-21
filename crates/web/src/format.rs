//! 숫자 표기: 금액은 원 단위 + 천 단위 구분, 큰 금액은 억/만 단위 축약.

pub fn int(n: i64) -> String {
    group(n.unsigned_abs(), n < 0)
}

/// 부호 없는 정수 (u64 전체 범위).
pub fn count(n: u64) -> String {
    group(n, false)
}

fn group(abs: u64, neg: bool) -> String {
    let s = abs.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3 + 1);
    if neg {
        out.push('-');
    }
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn won(n: i64) -> String {
    format!("{}원", int(n))
}

/// 1.3억 원, 4,520만 원, 9,800원
pub fn won_short(n: i64) -> String {
    let abs = n.unsigned_abs();
    if abs >= 100_000_000 {
        let v = n as f64 / 100_000_000.0;
        format!("{}억 원", trim_float(v, 1))
    } else if abs >= 10_000 {
        format!("{}만 원", int(n / 10_000))
    } else {
        won(n)
    }
}

pub fn pct(v: f64, digits: usize) -> String {
    if !v.is_finite() {
        return "–".into();
    }
    format!("{}%", trim_float(v * 100.0, digits))
}

/// 증감 표시: `▲ 12.3%`, `▼ 4%`, 변화 없음·정의 안 됨은 `–`. 반환 (문구, 방향: 1/0/-1)
pub fn delta_label(d: Option<f64>) -> (String, i8) {
    match d {
        Some(v) if v.is_finite() && (v * 1000.0).abs() >= 0.5 => {
            let p = v.abs() * 100.0;
            let s = if p >= 1000.0 {
                int(p.round() as i64)
            } else {
                trim_float(p, 1)
            };
            if v > 0.0 {
                (format!("▲ {s}%"), 1)
            } else {
                (format!("▼ {s}%"), -1)
            }
        }
        _ => ("–".into(), 0),
    }
}

/// ROAS: 3.25 → `325%`, None → `–`
pub fn roas(v: Option<f64>) -> String {
    match v {
        Some(r) if r.is_finite() => format!("{}%", int((r * 100.0).round() as i64)),
        _ => "–".into(),
    }
}

/// 큰 수 축약: 12,345 → `1.2만`, 9,999 → `9,999`, 1.5억
pub fn compact(n: u64) -> String {
    if n >= 100_000_000 {
        let v = n as f64 / 100_000_000.0;
        if v >= 1000.0 {
            format!("{}억", int(v.round() as i64))
        } else {
            format!("{}억", trim_float(v, 1))
        }
    } else if n >= 10_000 {
        format!("{}만", trim_float(n as f64 / 10_000.0, 1))
    } else {
        count(n)
    }
}

/// 유니코드 글자 기준 자르기 (넘치면 `…`).
pub fn truncate(s: &str, max_chars: usize) -> String {
    let mut it = s.chars();
    let head: String = it.by_ref().take(max_chars).collect();
    if it.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// 퍼센트 인코딩된 경로를 사람이 읽는 형태로 (화면 표시 전용, 텍스트로만 그린다).
pub fn path(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

pub(crate) fn trim_float(v: f64, digits: usize) -> String {
    let s = format!("{v:.digits$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    if s == "-0" { "0".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(int(1234567), "1,234,567");
        assert_eq!(int(-1000), "-1,000");
        assert_eq!(int(999), "999");
        assert_eq!(int(0), "0");
        assert_eq!(int(i64::MIN), "-9,223,372,036,854,775,808");
        assert_eq!(count(u64::MAX), "18,446,744,073,709,551,615");
        assert_eq!(won(100000), "100,000원");
        assert_eq!(won_short(130_000_000), "1.3억 원");
        assert_eq!(won_short(200_000_000), "2억 원");
        assert_eq!(won_short(45_200_000), "4,520만 원");
        assert_eq!(won_short(9_800), "9,800원");
        assert_eq!(won_short(-45_200_000), "-4,520만 원");
        assert_eq!(pct(0.35, 1), "35%");
        assert_eq!(pct(0.3254, 1), "32.5%");
        assert_eq!(pct(f64::NAN, 1), "–");
        assert_eq!(pct(-0.0001, 1), "0%");
    }

    #[test]
    fn deltas() {
        assert_eq!(delta_label(Some(0.123)), ("▲ 12.3%".into(), 1));
        assert_eq!(delta_label(Some(-0.04)), ("▼ 4%".into(), -1));
        assert_eq!(delta_label(Some(0.0)), ("–".into(), 0));
        assert_eq!(delta_label(Some(0.00001)), ("–".into(), 0));
        assert_eq!(delta_label(None), ("–".into(), 0));
        assert_eq!(delta_label(Some(f64::INFINITY)), ("–".into(), 0));
        assert_eq!(delta_label(Some(123.0)), ("▲ 12,300%".into(), 1));
    }

    #[test]
    fn roas_and_compact() {
        assert_eq!(roas(Some(3.25)), "325%");
        assert_eq!(roas(None), "–");
        assert_eq!(roas(Some(f64::NAN)), "–");
        assert_eq!(compact(0), "0");
        assert_eq!(compact(9_999), "9,999");
        assert_eq!(compact(12_345), "1.2만");
        assert_eq!(compact(150_000_000), "1.5억");
        assert_eq!(compact(1_000_000_000_000), "10,000억");
        assert!(compact(u64::MAX).ends_with('억'));
    }

    #[test]
    fn decodes_paths_for_display() {
        assert_eq!(path("/dev/%EC%83%81%ED%92%88"), "/dev/상품");
        assert_eq!(path("/a%20b?q=%3Cscript%3E"), "/a b?q=<script>");
        // 잘못된 인코딩·UTF-8이 아닌 바이트는 원문 유지
        assert_eq!(path("/bad%ZZ%"), "/bad%ZZ%");
        assert_eq!(path("/x%FF"), "/x%FF");
        assert_eq!(path(""), "");
    }

    #[test]
    fn truncation_is_unicode_safe() {
        assert_eq!(truncate("가나다라", 3), "가나다…");
        assert_eq!(truncate("가나다", 3), "가나다");
        assert_eq!(truncate("👍🏽ab", 1), "👍…");
        assert_eq!(truncate("", 3), "");
        assert_eq!(truncate("abc", 0), "…");
    }
}

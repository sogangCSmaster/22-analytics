//! 숫자 표기: 금액은 원 단위 + 천 단위 구분, 큰 금액은 억/만 단위 축약.

pub fn int(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3 + 1);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
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
    format!("{}%", trim_float(v * 100.0, digits))
}

fn trim_float(v: f64, digits: usize) -> String {
    let s = format!("{v:.digits$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(int(1234567), "1,234,567");
        assert_eq!(int(-1000), "-1,000");
        assert_eq!(int(999), "999");
        assert_eq!(won(100000), "100,000원");
        assert_eq!(won_short(130_000_000), "1.3억 원");
        assert_eq!(won_short(200_000_000), "2억 원");
        assert_eq!(won_short(45_200_000), "4,520만 원");
        assert_eq!(won_short(9_800), "9,800원");
        assert_eq!(pct(0.35, 1), "35%");
        assert_eq!(pct(0.3254, 1), "32.5%");
    }
}

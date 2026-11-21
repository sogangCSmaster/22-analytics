//! 폼 입력 검증 (서버 검증과 같은 규칙을 먼저 화면에서 알려준다).

use analytics_core::channel::RuleMatch;
use analytics_core::dto::admin::CreateChannelRuleRequest;

pub const MIN_PASSWORD: usize = 10;
pub const MAX_PASSWORD: usize = 256;
pub const MAX_NAME: usize = 200;
pub const MAX_CSV_BYTES: u64 = 5 * 1024 * 1024;

pub fn email(s: &str) -> Result<String, &'static str> {
    let e = s.trim().to_lowercase();
    let bad = Err("올바른 이메일 주소를 입력하세요");
    if e.is_empty() || e.len() > 254 || e.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return bad;
    }
    let mut parts = e.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return bad;
    };
    if local.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.contains("..")
    {
        return bad;
    }
    Ok(e)
}

pub fn password(s: &str) -> Result<(), &'static str> {
    let n = s.chars().count();
    if (MIN_PASSWORD..=MAX_PASSWORD).contains(&n) {
        Ok(())
    } else {
        Err("비밀번호는 10자 이상이어야 합니다")
    }
}

/// 이름류: 앞뒤 공백 제거 후 1..=200자, 제어 문자 금지.
pub fn name(s: &str) -> Result<String, &'static str> {
    let t = s.trim();
    if t.is_empty() {
        return Err("이름을 입력하세요");
    }
    if t.chars().count() > MAX_NAME {
        return Err("이름은 200자 이하로 입력하세요");
    }
    if t.chars().any(char::is_control) {
        return Err("사용할 수 없는 문자가 있습니다");
    }
    Ok(t.to_owned())
}

/// 허용 도메인. 마케터가 붙여 넣는 `https://Shop.com/path` 같은 값은 도메인만 남긴다.
pub fn origin(s: &str) -> Result<String, &'static str> {
    let bad = Err("도메인만 입력하세요 (예: shop.client.com)");
    let mut t = s.trim().to_lowercase();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = t.strip_prefix(scheme) {
            t = rest.to_owned();
        }
    }
    // 경로·쿼리는 잘라낸다 (붙여 넣은 URL)
    if let Some(i) = t.find(['/', '?', '#']) {
        t.truncate(i);
    }
    let t = t.strip_suffix('.').unwrap_or(&t).to_owned();
    if t.is_empty() || t.len() > 253 {
        return bad;
    }
    let labels: Vec<&str> = t.split('.').collect();
    if labels.len() < 2 && t != "localhost" {
        return bad;
    }
    let label_ok = |l: &&str| {
        (1..=63).contains(&l.len())
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    if labels.iter().all(label_ok) {
        Ok(t)
    } else {
        bad
    }
}

/// 쉼표·줄바꿈 구분 목록: 공백 제거, 빈 값 제거, 중복 제거 (순서 유지).
pub fn split_list(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in s.split([',', '\n', '\r']) {
        let p = p.trim();
        if !p.is_empty() && !out.iter().any(|x| x == p) {
            out.push(p.to_owned());
        }
    }
    out
}

/// 쿼리 파라미터 이름 목록.
pub fn param_list(s: &str) -> Result<Vec<String>, String> {
    let list = split_list(s);
    if list.len() > 50 {
        return Err("최대 50개까지 입력할 수 있습니다".into());
    }
    for p in &list {
        let ok = p.len() <= 100
            && p.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'[' | b']')
            });
        if !ok {
            return Err(format!(
                "'{p}': 파라미터 이름은 영문·숫자·_-.[] 만 쓸 수 있습니다"
            ));
        }
    }
    Ok(list)
}

/// 채널 규칙 입력.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleForm {
    pub channel: String,
    pub utm_source: String,
    pub utm_medium: String,
    pub utm_campaign_prefix: String,
    pub click_id_type: String,
    pub referrer_domain: String,
    pub priority: String,
    pub label: String,
}

pub fn rule(f: &RuleForm) -> Result<CreateChannelRuleRequest, &'static str> {
    let channel = f.channel.trim();
    if channel.is_empty() {
        return Err("채널 이름을 입력하세요");
    }
    if channel.chars().count() > 100 {
        return Err("채널 이름은 100자 이하로 입력하세요");
    }
    let when = RuleMatch {
        utm_source: split_list(&f.utm_source),
        utm_medium: split_list(&f.utm_medium),
        utm_campaign_prefix: split_list(&f.utm_campaign_prefix),
        click_id_type: split_list(&f.click_id_type),
        referrer_domain: split_list(&f.referrer_domain),
    };
    if when == RuleMatch::default() {
        return Err("조건을 하나 이상 입력하세요");
    }
    let priority = match f.priority.trim() {
        "" => None,
        p => match p.parse::<i32>() {
            Ok(n) if (0..=100_000).contains(&n) => Some(n),
            _ => return Err("우선순위는 0~100000 사이 숫자입니다"),
        },
    };
    Ok(CreateChannelRuleRequest {
        channel: channel.to_owned(),
        when,
        priority,
        label: f.label.trim().to_owned(),
    })
}

/// `7, 30` → [7, 30] (1..=90, 정렬·중복 제거, 1~10개)
pub fn lookbacks(s: &str) -> Result<Vec<i16>, &'static str> {
    let mut v: Vec<i16> = Vec::new();
    for p in split_list(s) {
        match p.parse::<i16>() {
            Ok(n) if (1..=90).contains(&n) => v.push(n),
            _ => return Err("기간은 1~90 사이 숫자(일)로 입력하세요"),
        }
    }
    v.sort_unstable();
    v.dedup();
    match v.len() {
        0 => Err("기간을 하나 이상 입력하세요"),
        1..=10 => Ok(v),
        _ => Err("기간은 최대 10개까지 입력할 수 있습니다"),
    }
}

/// 1..=2160 시간
pub fn half_life(s: &str) -> Result<i32, &'static str> {
    match s.trim().parse::<i32>() {
        Ok(h) if (1..=2160).contains(&h) => Ok(h),
        _ => Err("반감기는 1~2160 시간 사이로 입력하세요"),
    }
}

/// 광고비 CSV 파일 1차 확인 (이름·크기·머리글).
pub fn ad_spend_csv(file_name: &str, size: u64, text: &str) -> Result<usize, &'static str> {
    if !file_name.to_ascii_lowercase().ends_with(".csv") {
        return Err("CSV 파일(.csv)만 올릴 수 있습니다");
    }
    if size == 0 {
        return Err("빈 파일입니다");
    }
    if size > MAX_CSV_BYTES {
        return Err("파일은 최대 5MB까지 올릴 수 있습니다");
    }
    let mut rows = 0;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || (i == 0 && line.to_ascii_lowercase().starts_with("day")) {
            continue;
        }
        if line.split(',').count() < 4 {
            return Err("열은 day,channel,campaign,amount 4개여야 합니다");
        }
        rows += 1;
    }
    if rows == 0 {
        return Err("데이터 행이 없습니다");
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails() {
        assert_eq!(email(" Admin@Example.com "), Ok("admin@example.com".into()));
        for bad in [
            "",
            "a",
            "a@b",
            "@b.com",
            "a@.com",
            "a@b.com.",
            "a b@c.com",
            "a@b..com",
            "a@b@c.com",
        ] {
            assert!(email(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn passwords() {
        assert!(password("123456789").is_err());
        assert!(password("1234567890").is_ok());
        assert!(password("가나다라마바사아자차").is_ok()); // 글자 수 기준
        assert!(password(&"x".repeat(257)).is_err());
    }

    #[test]
    fn names() {
        assert_eq!(name("  광고주 A "), Ok("광고주 A".into()));
        assert!(name("   ").is_err());
        assert!(name("a\u{0007}b").is_err());
        assert!(name(&"가".repeat(201)).is_err());
        assert!(name(&"가".repeat(200)).is_ok());
    }

    #[test]
    fn origins() {
        assert_eq!(origin("shop.client.com"), Ok("shop.client.com".into()));
        assert_eq!(
            origin(" https://Shop.Client.com/path?x=1 "),
            Ok("shop.client.com".into())
        );
        assert_eq!(origin("http://localhost"), Ok("localhost".into()));
        assert_eq!(origin("client.com."), Ok("client.com".into()));
        for bad in [
            "",
            "client",
            "*.client.com",
            "client.com:8080",
            "a b.com",
            "<script>.com",
            "-a.com",
            "a..com",
            "user@client.com",
        ] {
            assert!(origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn lists() {
        assert_eq!(split_list(" a, b\n\nc ,a,"), vec!["a", "b", "c"]);
        assert!(split_list("  ").is_empty());
        assert_eq!(
            param_list("gclid, fbclid"),
            Ok(vec!["gclid".into(), "fbclid".into()])
        );
        assert!(param_list("utm source").is_err());
        assert!(param_list("a=b").is_err());
    }

    #[test]
    fn rules() {
        let mut f = RuleForm {
            channel: "naver_shopping".into(),
            utm_source: "naver, NAVER2".into(),
            ..Default::default()
        };
        let r = rule(&f).unwrap();
        assert_eq!(r.channel, "naver_shopping");
        assert_eq!(r.when.utm_source, vec!["naver", "NAVER2"]);
        assert_eq!(r.priority, None);
        f.priority = "10".into();
        assert_eq!(rule(&f).unwrap().priority, Some(10));
        f.priority = "-1".into();
        assert_eq!(rule(&f).unwrap_err(), "우선순위는 0~100000 사이 숫자입니다");
        f.priority = "".into();
        f.utm_source = "".into();
        assert_eq!(rule(&f).unwrap_err(), "조건을 하나 이상 입력하세요");
        f.utm_medium = "cpc".into();
        f.channel = " ".into();
        assert_eq!(rule(&f).unwrap_err(), "채널 이름을 입력하세요");
    }

    #[test]
    fn lookback_parsing() {
        assert_eq!(lookbacks("30, 7, 7"), Ok(vec![7, 30]));
        assert!(lookbacks("").is_err());
        assert!(lookbacks("0").is_err());
        assert!(lookbacks("91").is_err());
        assert!(lookbacks("7일").is_err());
        assert!(lookbacks("1,2,3,4,5,6,7,8,9,10,11").is_err());
        assert_eq!(half_life("168"), Ok(168));
        assert!(half_life("0").is_err());
        assert!(half_life("2161").is_err());
        assert!(half_life("abc").is_err());
    }

    #[test]
    fn csv_check() {
        let ok = "day,channel,campaign,amount\n2026-11-01,meta,brand,10000\n";
        assert_eq!(ad_spend_csv("spend.csv", 100, ok), Ok(1));
        assert_eq!(
            ad_spend_csv(
                "spend.CSV",
                100,
                "2026-11-01,meta,brand,1\n\n2026-11-02,meta,brand,1"
            ),
            Ok(2)
        );
        assert!(ad_spend_csv("spend.xlsx", 100, ok).is_err());
        assert!(ad_spend_csv("spend.csv", MAX_CSV_BYTES + 1, ok).is_err());
        assert!(ad_spend_csv("spend.csv", 0, "").is_err());
        assert!(ad_spend_csv("spend.csv", 10, "day,channel,campaign,amount\n").is_err());
        assert!(ad_spend_csv("spend.csv", 10, "a,b\n").is_err());
    }
}

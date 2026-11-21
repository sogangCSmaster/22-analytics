//! 입력 검증. 실패 메시지는 사용자에게 그대로 보이므로 내부 정보를 넣지 않는다.

use crate::error::{ApiError, ApiResult};
use analytics_core::channel::RuleMatch;
use analytics_core::settings::TrackingSettings;

pub const MAX_NAME_CHARS: usize = 200;
pub const MAX_MEMO_CHARS: usize = 2000;
pub const MIN_PASSWORD_CHARS: usize = 10;
pub const MAX_PASSWORD_CHARS: usize = 256;
pub const MAX_EMAIL_BYTES: usize = 254;

/// 이름류: 앞뒤 공백 제거, 1..=max 글자(유니코드 문자 기준), 제어 문자 금지.
pub fn name(field: &str, s: &str, max: usize) -> ApiResult<String> {
    let t = s.trim();
    if t.is_empty() {
        return Err(ApiError::invalid(format!("{field}: 비어 있을 수 없습니다")));
    }
    if t.chars().count() > max {
        return Err(ApiError::invalid(format!("{field}: 최대 {max}자")));
    }
    if t.chars().any(char::is_control) {
        return Err(ApiError::invalid(format!(
            "{field}: 제어 문자는 쓸 수 없습니다"
        )));
    }
    Ok(t.to_owned())
}

/// 비어 있어도 되는 이름 (사용자 이름).
pub fn optional_name(field: &str, s: &str, max: usize) -> ApiResult<String> {
    if s.trim().is_empty() {
        Ok(String::new())
    } else {
        name(field, s, max)
    }
}

/// 메모: 줄바꿈·탭은 허용, 그 밖의 제어 문자 금지.
pub fn memo(s: &str) -> ApiResult<String> {
    let t = s.trim();
    if t.chars().count() > MAX_MEMO_CHARS {
        return Err(ApiError::invalid(format!("memo: 최대 {MAX_MEMO_CHARS}자")));
    }
    if t.chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(ApiError::invalid("memo: 제어 문자는 쓸 수 없습니다"));
    }
    Ok(t.to_owned())
}

/// 로그인용 이메일 정규화 (형식 검사 없음): trim + 소문자.
pub fn normalize_email(s: &str) -> String {
    s.trim().to_lowercase()
}

/// 계정 생성용 이메일 검사.
pub fn email(s: &str) -> ApiResult<String> {
    let e = normalize_email(s);
    let bad = || ApiError::invalid("email: 올바른 이메일 주소가 아닙니다");
    if e.is_empty() || e.len() > MAX_EMAIL_BYTES {
        return Err(bad());
    }
    if e.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(bad());
    }
    let mut parts = e.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad());
    };
    if local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.contains("..")
    {
        return Err(bad());
    }
    Ok(e)
}

pub fn password(s: &str) -> ApiResult<()> {
    let n = s.chars().count();
    if !(MIN_PASSWORD_CHARS..=MAX_PASSWORD_CHARS).contains(&n) {
        return Err(ApiError::invalid(format!(
            "password: {MIN_PASSWORD_CHARS}~{MAX_PASSWORD_CHARS}자"
        )));
    }
    Ok(())
}

/// ISO 4217 형식 (대문자 3자). 실제 통화 코드 목록 대조는 하지 않는다.
pub fn currency(s: &str) -> ApiResult<String> {
    let t = s.trim();
    if t.len() == 3 && t.bytes().all(|b| b.is_ascii_uppercase()) {
        Ok(t.to_owned())
    } else {
        Err(ApiError::invalid("currency: ISO 4217 대문자 3자 (예: KRW)"))
    }
}

/// 타임존 형식 1차 검사 (DB `pg_timezone_names` 대조 전에 터무니없는 값을 거른다).
pub fn timezone_shape(s: &str) -> ApiResult<String> {
    let t = s.trim();
    let ok = !t.is_empty()
        && t.len() <= 64
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'));
    if ok {
        Ok(t.to_owned())
    } else {
        Err(ApiError::invalid("timezone: IANA 타임존 이름이 아닙니다"))
    }
}

/// 허용 도메인 정규화: 스킴·경로·포트·와일드카드 없는 도메인 또는 IPv4.
/// 소문자로 바꾸고, 유니코드 도메인은 punycode(`xn--`)로 바꾼다. 끝의 `.` 하나는 무시한다.
pub fn origin(s: &str) -> ApiResult<String> {
    let bad = || {
        ApiError::invalid("origin: 스킴·경로·포트 없는 도메인만 입력하세요 (예: shop.client.com)")
    };
    let t = s.trim();
    let t = t.strip_suffix('.').unwrap_or(t);
    if t.is_empty() || t.len() > 253 * 4 {
        return Err(bad());
    }
    if t.chars().any(|c| {
        c.is_whitespace()
            || c.is_control()
            || matches!(
                c,
                '/' | '\\' | ':' | '@' | '?' | '#' | '*' | '[' | ']' | '%' | '<' | '>' | '\'' | '"'
            )
    }) {
        return Err(bad());
    }
    if let Ok(ip) = t.parse::<std::net::Ipv4Addr>() {
        return if ip.to_string() == t {
            Ok(t.to_owned())
        } else {
            Err(bad())
        };
    }
    let host = match url::Host::parse(t) {
        Ok(url::Host::Domain(d)) => d,
        // "1.2.3" 같은 축약 IPv4나 IPv6는 허용하지 않는다.
        _ => return Err(bad()),
    };
    if host.len() > 253 {
        return Err(bad());
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 && host != "localhost" {
        return Err(bad());
    }
    for l in &labels {
        let ok = (1..=63).contains(&l.len())
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-');
        if !ok {
            return Err(bad());
        }
    }
    Ok(host)
}

const MAX_PARAMS: usize = 50;
const MAX_PARAM_CHARS: usize = 100;

fn param_list(field: &str, list: &[String]) -> ApiResult<Vec<String>> {
    if list.len() > MAX_PARAMS {
        return Err(ApiError::invalid(format!("{field}: 최대 {MAX_PARAMS}개")));
    }
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    for p in list {
        let p = p.trim();
        let ok = !p.is_empty()
            && p.len() <= MAX_PARAM_CHARS
            && p.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'[' | b']')
            });
        if !ok {
            return Err(ApiError::invalid(format!(
                "{field}: 파라미터 이름은 1~{MAX_PARAM_CHARS}자 영문·숫자·_-.[]"
            )));
        }
        if !out.iter().any(|x| x == p) {
            out.push(p.to_owned());
        }
    }
    Ok(out)
}

/// 수집 설정 검증·정규화 (파라미터 목록 trim·중복 제거).
pub fn tracking_settings(mut s: TrackingSettings) -> ApiResult<TrackingSettings> {
    s.click_id_params = param_list("click_id_params", &s.click_id_params)?;
    s.capture_query_params = param_list("capture_query_params", &s.capture_query_params)?;
    s.strip_query_params = param_list("strip_query_params", &s.strip_query_params)?;
    Ok(s)
}

pub const MAX_CHANNEL_CHARS: usize = 100;
const MAX_RULE_VALUES: usize = 50;
const MAX_RULE_VALUE_CHARS: usize = 200;
pub const MAX_PRIORITY: i32 = 100_000;

/// 채널 규칙 조건 검증: 조건이 하나 이상, 각 값은 비어 있지 않음.
pub fn rule_match(m: RuleMatch) -> ApiResult<RuleMatch> {
    fn list(field: &str, v: Vec<String>) -> ApiResult<Vec<String>> {
        if v.len() > MAX_RULE_VALUES {
            return Err(ApiError::invalid(format!(
                "match.{field}: 최대 {MAX_RULE_VALUES}개"
            )));
        }
        v.into_iter()
            .map(|x| name(&format!("match.{field}"), &x, MAX_RULE_VALUE_CHARS))
            .collect()
    }
    let m = RuleMatch {
        utm_source: list("utm_source", m.utm_source)?,
        utm_medium: list("utm_medium", m.utm_medium)?,
        utm_campaign_prefix: list("utm_campaign_prefix", m.utm_campaign_prefix)?,
        click_id_type: list("click_id_type", m.click_id_type)?,
        referrer_domain: list("referrer_domain", m.referrer_domain)?,
    };
    let empty = m.utm_source.is_empty()
        && m.utm_medium.is_empty()
        && m.utm_campaign_prefix.is_empty()
        && m.click_id_type.is_empty()
        && m.referrer_domain.is_empty();
    if empty {
        return Err(ApiError::invalid("match: 조건을 하나 이상 지정하세요"));
    }
    Ok(m)
}

pub fn priority(p: i32) -> ApiResult<i32> {
    if (0..=MAX_PRIORITY).contains(&p) {
        Ok(p)
    } else {
        Err(ApiError::invalid(format!("priority: 0~{MAX_PRIORITY}")))
    }
}

pub const MAX_LOOKBACK_DAYS: i16 = 90;
pub const MAX_LOOKBACKS: usize = 10;
pub const MAX_HALF_LIFE_HOURS: i32 = 2160;

/// lookback 목록: 1..=90, 정렬·중복 제거 후 1~10개.
pub fn lookbacks(v: &[i16]) -> ApiResult<Vec<i16>> {
    if v.iter().any(|d| !(1..=MAX_LOOKBACK_DAYS).contains(d)) {
        return Err(ApiError::invalid(format!(
            "lookback_days: 1~{MAX_LOOKBACK_DAYS}일"
        )));
    }
    let mut out = v.to_vec();
    out.sort_unstable();
    out.dedup();
    if out.is_empty() || out.len() > MAX_LOOKBACKS {
        return Err(ApiError::invalid(format!(
            "lookback_days: 1~{MAX_LOOKBACKS}개"
        )));
    }
    Ok(out)
}

pub fn half_life(h: i32) -> ApiResult<i32> {
    if (1..=MAX_HALF_LIFE_HOURS).contains(&h) {
        Ok(h)
    } else {
        Err(ApiError::invalid(format!(
            "half_life_hours: 1~{MAX_HALF_LIFE_HOURS}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(name("n", "  가나 ", 200).unwrap(), "가나");
        assert!(name("n", "", 200).is_err());
        assert!(name("n", "가".repeat(201).as_str(), 200).is_err());
        assert!(name("n", "가".repeat(200).as_str(), 200).is_ok());
        assert!(name("n", "a\u{7}b", 200).is_err());
        assert_eq!(optional_name("n", "  ", 200).unwrap(), "");
    }

    #[test]
    fn emails() {
        assert_eq!(email(" A@B.co ").unwrap(), "a@b.co");
        for bad in [
            "", "a", "a@", "@b.c", "a@b", "a@@b.c", "a b@c.d", "a@.b", "a@b..c",
        ] {
            assert!(email(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn origins() {
        assert_eq!(origin("Shop.Client.com.").unwrap(), "shop.client.com");
        assert_eq!(origin("한국.kr").unwrap(), "xn--3e0b707e.kr");
        assert_eq!(origin("localhost").unwrap(), "localhost");
        assert_eq!(origin("127.0.0.1").unwrap(), "127.0.0.1");
        for bad in [
            "http://a.com",
            "a.com/x",
            "a.com:80",
            "*.a.com",
            "a",
            "127.1",
            "1.2.3",
            "[::1]",
            "a_b.com",
            "%61.com",
            "-a.com",
        ] {
            assert!(origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn lookback_rules() {
        assert_eq!(lookbacks(&[30, 7, 7]).unwrap(), vec![7, 30]);
        assert!(lookbacks(&[]).is_err());
        assert!(lookbacks(&[0]).is_err());
        assert!(lookbacks(&[91]).is_err());
        assert!(lookbacks(&[1, 90]).is_ok());
    }

    #[test]
    fn rule_match_requires_condition() {
        assert!(rule_match(RuleMatch::default()).is_err());
        let m = rule_match(RuleMatch {
            utm_source: vec![" naver ".into()],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(m.utm_source, vec!["naver"]);
    }
}

//! 폼 입력 검증 (서버 검증과 같은 규칙을 먼저 화면에서 알려준다).

use analytics_core::channel::RuleMatch;
use analytics_core::dto::admin::CreateChannelRuleRequest;

pub const MIN_PASSWORD: usize = 10;
pub const MAX_PASSWORD: usize = 256;
pub const MAX_NAME: usize = 200;
pub const MAX_CSV_BYTES: u64 = 5 * 1024 * 1024;

pub fn email(s: &str) -> Result<String, &'static str> {
    todo!()
}

pub fn password(s: &str) -> Result<(), &'static str> {
    todo!()
}

/// 이름류: 앞뒤 공백 제거 후 1..=200자, 제어 문자 금지.
pub fn name(s: &str) -> Result<String, &'static str> {
    todo!()
}

/// 허용 도메인. 마케터가 붙여 넣는 `https://Shop.com/path` 같은 값은 도메인만 남긴다.
pub fn origin(s: &str) -> Result<String, &'static str> {
    todo!()
}

/// 쉼표·줄바꿈 구분 목록: 공백 제거, 빈 값 제거, 중복 제거 (순서 유지).
pub fn split_list(s: &str) -> Vec<String> {
    todo!()
}

/// 쿼리 파라미터 이름 목록.
pub fn param_list(s: &str) -> Result<Vec<String>, String> {
    todo!()
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
    todo!()
}

/// `7, 30` → [7, 30] (1..=90, 정렬·중복 제거, 1~10개)
pub fn lookbacks(s: &str) -> Result<Vec<i16>, &'static str> {
    todo!()
}

/// 1..=2160 시간
pub fn half_life(s: &str) -> Result<i32, &'static str> {
    todo!()
}

/// 광고비 CSV 파일 1차 확인 (이름·크기·머리글).
pub fn ad_spend_csv(file_name: &str, size: u64, text: &str) -> Result<usize, &'static str> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails() {
        assert_eq!(email(" Admin@Example.com "), Ok("admin@example.com".into()));
        for bad in ["", "a", "a@b", "@b.com", "a@.com", "a@b.com.", "a b@c.com", "a@b..com", "a@b@c.com"] {
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
        assert_eq!(origin(" https://Shop.Client.com/path?x=1 "), Ok("shop.client.com".into()));
        assert_eq!(origin("http://localhost"), Ok("localhost".into()));
        assert_eq!(origin("client.com."), Ok("client.com".into()));
        for bad in ["", "client", "*.client.com", "client.com:8080", "a b.com", "<script>.com", "-a.com", "a..com", "user@client.com"] {
            assert!(origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn lists() {
        assert_eq!(split_list(" a, b\n\nc ,a,"), vec!["a", "b", "c"]);
        assert!(split_list("  ").is_empty());
        assert_eq!(param_list("gclid, fbclid"), Ok(vec!["gclid".into(), "fbclid".into()]));
        assert!(param_list("utm source").is_err());
        assert!(param_list("a=b").is_err());
    }

    #[test]
    fn rules() {
        let mut f = RuleForm { channel: "naver_shopping".into(), utm_source: "naver, NAVER2".into(), ..Default::default() };
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
        assert_eq!(ad_spend_csv("spend.CSV", 100, "2026-11-01,meta,brand,1\n\n2026-11-02,meta,brand,1"), Ok(2));
        assert!(ad_spend_csv("spend.xlsx", 100, ok).is_err());
        assert!(ad_spend_csv("spend.csv", MAX_CSV_BYTES + 1, ok).is_err());
        assert!(ad_spend_csv("spend.csv", 0, "").is_err());
        assert!(ad_spend_csv("spend.csv", 10, "day,channel,campaign,amount\n").is_err());
        assert!(ad_spend_csv("spend.csv", 10, "a,b\n").is_err());
    }
}

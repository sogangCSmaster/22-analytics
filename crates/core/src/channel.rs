//! 채널 정규화.
//!
//! 원본 값(utm, 클릭 ID 종류, referrer)은 그대로 저장하고, 채널 이름은 규칙으로 계산한다.
//! 프로젝트 규칙을 먼저 평가하고, 일치하는 게 없으면 기본 규칙을 쓴다.

use crate::urlx::{registrable_domain, same_site};
use serde::{Deserialize, Serialize};

pub const DIRECT: &str = "direct";
pub const INTERNAL: &str = "(internal)";
pub const ORGANIC_SEARCH: &str = "organic_search";
pub const ORGANIC_SOCIAL: &str = "organic_social";
pub const REFERRAL: &str = "referral";
pub const UNATTRIBUTED: &str = "(unattributed)";

/// 채널 판정 입력.
#[derive(Debug, Default, Clone)]
pub struct ChannelInput<'a> {
    pub utm_source: &'a str,
    pub utm_medium: &'a str,
    pub utm_campaign: &'a str,
    pub click_id_type: &'a str,
    pub referrer_host: &'a str,
    pub page_host: &'a str,
}

/// 프로젝트 채널 규칙. 비어 있지 않은 조건은 모두 만족해야 하고(AND),
/// 각 조건 목록 안에서는 하나만 맞으면 된다(OR). 비교는 대소문자·앞뒤 공백 무시.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuleMatch {
    pub utm_source: Vec<String>,
    pub utm_medium: Vec<String>,
    pub utm_campaign_prefix: Vec<String>,
    pub click_id_type: Vec<String>,
    pub referrer_domain: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelRule {
    pub channel: String,
    #[serde(rename = "match")]
    pub when: RuleMatch,
}

impl RuleMatch {
    fn is_empty(&self) -> bool {
        self.utm_source.is_empty()
            && self.utm_medium.is_empty()
            && self.utm_campaign_prefix.is_empty()
            && self.click_id_type.is_empty()
            && self.referrer_domain.is_empty()
    }

    pub fn matches(&self, i: &ChannelInput<'_>) -> bool {
        if self.is_empty() {
            return false;
        }
        let any_eq =
            |list: &[String], v: &str| list.is_empty() || list.iter().any(|x| eq_norm(x, v));
        let campaign = norm(i.utm_campaign);
        any_eq(&self.utm_source, i.utm_source)
            && any_eq(&self.utm_medium, i.utm_medium)
            && any_eq(&self.click_id_type, i.click_id_type)
            && (self.utm_campaign_prefix.is_empty()
                || self
                    .utm_campaign_prefix
                    .iter()
                    .any(|p| campaign.starts_with(&norm(p))))
            && (self.referrer_domain.is_empty()
                || (!i.referrer_host.is_empty()
                    && self
                        .referrer_domain
                        .iter()
                        .any(|d| registrable_domain(i.referrer_host) == norm(d))))
    }
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

fn eq_norm(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// 검색엔진 (등록 가능 도메인 기준).
const SEARCH_ENGINES: &[&str] = &[
    "naver.com",
    "google.com",
    "google.co.kr",
    "daum.net",
    "bing.com",
    "yahoo.com",
    "zum.com",
    "duckduckgo.com",
    "baidu.com",
    "yandex.ru",
    "ecosia.org",
];

const SOCIAL: &[&str] = &[
    "facebook.com",
    "instagram.com",
    "t.co",
    "x.com",
    "twitter.com",
    "youtube.com",
    "tiktok.com",
    "linkedin.com",
    "pinterest.com",
    "threads.net",
    "kakao.com",
    "band.us",
    "blog.naver.com",
];

/// 채널을 판정한다.
pub fn classify(input: &ChannelInput<'_>, project_rules: &[ChannelRule]) -> String {
    if let Some(r) = project_rules.iter().find(|r| r.when.matches(input)) {
        return r.channel.clone();
    }
    default_channel(input)
}

fn default_channel(i: &ChannelInput<'_>) -> String {
    let src = norm(i.utm_source);
    let med = norm(i.utm_medium);
    let paid_medium = matches!(
        med.as_str(),
        "cpc" | "ppc" | "sa" | "paid" | "paid_search" | "powerlink" | "keyword" | "search"
    );

    // 1. 클릭 ID가 매체를 확정해 주는 경우
    match i.click_id_type {
        "gclid" | "wbraid" | "gbraid" => return "google_ads".into(),
        "fbclid" => return "meta".into(),
        "ttclid" => return "tiktok".into(),
        "n_keyword" | "NaPm" => return "naver_sa".into(),
        _ => {}
    }

    // 2. utm_source 기반
    if !src.is_empty() {
        return match src.as_str() {
            "facebook" | "fb" | "instagram" | "ig" | "meta" => "meta".into(),
            "naver" if paid_medium => "naver_sa".into(),
            "naver" | "naverblog" | "naver_blog" => "naver".into(),
            "google" if paid_medium => "google_ads".into(),
            "google" => "google".into(),
            "kakao" | "kakaomoment" | "kakao_moment" | "kakaotalk" => "kakao".into(),
            "tiktok" => "tiktok".into(),
            "criteo" => "criteo".into(),
            "youtube" => "youtube".into(),
            other => sanitize_channel(other),
        };
    }

    // 3. utm 없이 MMP/키워드 트래커 클릭 ID만 있는 경우
    match i.click_id_type {
        "abx_tid" => return "dfinery".into(),
        "_a1_kad" => return "a1_keyword".into(),
        "" => {}
        _ => return "(paid_unknown)".into(),
    }

    // 4. referrer 기반
    if i.referrer_host.is_empty() {
        return DIRECT.into();
    }
    if !i.page_host.is_empty() && same_site(i.referrer_host, i.page_host) {
        return INTERNAL.into();
    }
    let host = i.referrer_host.to_ascii_lowercase();
    let reg = registrable_domain(&host);
    if host.starts_with("blog.naver.com")
        || host.starts_with("m.blog.naver.com")
        || host.starts_with("cafe.naver.com")
    {
        return ORGANIC_SOCIAL.into();
    }
    if SEARCH_ENGINES.contains(&reg.as_str()) || reg.starts_with("google.") {
        return ORGANIC_SEARCH.into();
    }
    if SOCIAL.contains(&reg.as_str()) {
        return ORGANIC_SOCIAL.into();
    }
    REFERRAL.into()
}

/// 임의 utm_source를 채널 이름으로 정리: 소문자, 공백·특수문자 → `_`, 최대 64자.
fn sanitize_channel(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(64);
    out
}

/// 터치포인트(기여 대상 유입)인지: direct와 내부 이동은 제외한다.
pub fn is_touchpoint_channel(channel: &str) -> bool {
    channel != DIRECT && channel != INTERNAL
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ci<'a>(src: &'a str, med: &'a str, cid: &'a str, refh: &'a str) -> ChannelInput<'a> {
        ChannelInput {
            utm_source: src,
            utm_medium: med,
            click_id_type: cid,
            referrer_host: refh,
            page_host: "www.client.com",
            ..Default::default()
        }
    }

    #[test]
    fn utm_case_variants_unify() {
        for s in ["naver", "Naver", "NAVER", " naver "] {
            assert_eq!(classify(&ci(s, "cpc", "", ""), &[]), "naver_sa");
        }
    }

    #[test]
    fn click_ids() {
        assert_eq!(classify(&ci("", "", "gclid", ""), &[]), "google_ads");
        assert_eq!(classify(&ci("", "", "fbclid", ""), &[]), "meta");
        assert_eq!(classify(&ci("", "", "abx_tid", ""), &[]), "dfinery");
        // utm이 있으면 MMP 클릭 ID보다 utm이 매체를 더 잘 설명한다.
        assert_eq!(
            classify(&ci("naver", "cpc", "abx_tid", ""), &[]),
            "naver_sa"
        );
    }

    #[test]
    fn referrers() {
        assert_eq!(
            classify(&ci("", "", "", "m.search.naver.com"), &[]),
            "organic_search"
        );
        assert_eq!(
            classify(&ci("", "", "", "www.google.co.kr"), &[]),
            "organic_search"
        );
        assert_eq!(
            classify(&ci("", "", "", "blog.naver.com"), &[]),
            "organic_social"
        );
        assert_eq!(
            classify(&ci("", "", "", "news.example.org"), &[]),
            "referral"
        );
        assert_eq!(classify(&ci("", "", "", "shop.client.com"), &[]), INTERNAL);
        assert_eq!(classify(&ci("", "", "", ""), &[]), DIRECT);
    }

    #[test]
    fn project_rules_win() {
        let rules = vec![ChannelRule {
            channel: "naver_shopping".into(),
            when: RuleMatch {
                utm_source: vec!["naver".into()],
                utm_campaign_prefix: vec!["shop_".into()],
                ..Default::default()
            },
        }];
        let mut i = ci("NAVER", "cpc", "", "");
        i.utm_campaign = "SHOP_winter";
        assert_eq!(classify(&i, &rules), "naver_shopping");
        i.utm_campaign = "brand";
        assert_eq!(classify(&i, &rules), "naver_sa");
    }

    #[test]
    fn empty_rule_never_matches() {
        let rules = vec![ChannelRule {
            channel: "x".into(),
            when: RuleMatch::default(),
        }];
        assert_eq!(classify(&ci("", "", "", ""), &rules), DIRECT);
    }

    #[test]
    fn unknown_source_is_sanitized() {
        assert_eq!(classify(&ci("My Partner!", "", "", ""), &[]), "my_partner_");
    }
}

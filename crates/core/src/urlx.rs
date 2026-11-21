//! URL 파싱과 정리: 캠페인 파라미터 추출, 민감 파라미터 제거, 도메인 비교.

use crate::settings::TrackingSettings;
use url::Url;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Campaign {
    pub utm_source: String,
    pub utm_medium: String,
    pub utm_campaign: String,
    pub utm_content: String,
    pub utm_term: String,
    pub click_id: String,
    pub click_id_type: String,
}

impl Campaign {
    pub fn is_empty(&self) -> bool {
        self.utm_source.is_empty() && self.click_id.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageUrl {
    /// 민감 파라미터를 제거한 URL
    pub url: String,
    pub host: String,
    pub path: String,
    pub campaign: Campaign,
}

/// 페이지 URL을 파싱하고 정리한다. http(s)가 아니면 `None`.
///
/// 수집 경로에서 이벤트마다 호출되므로 할당을 줄인다: 키는 빌린 채로 비교하고(대소문자 무시),
/// 제거할 파라미터가 없으면 원래 쿼리 문자열을 그대로 둔다(다시 인코딩하지 않는다).
pub fn parse_page_url(raw: &str, settings: &TrackingSettings) -> Option<PageUrl> {
    let mut url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    // http(s)는 파서가 host를 이미 소문자로 정규화한다.
    let host = url.host_str()?.to_owned();
    let mut campaign = Campaign::default();
    let mut stripped = false;
    let mut kept = 0usize;

    for (k, v) in url.query_pairs() {
        if settings.should_strip(&k) {
            stripped = true;
            continue;
        }
        kept += 1;
        match &*k {
            "utm_source" => campaign.utm_source = clip(&v, 200),
            "utm_medium" => campaign.utm_medium = clip(&v, 200),
            "utm_campaign" => campaign.utm_campaign = clip(&v, 200),
            "utm_content" => campaign.utm_content = clip(&v, 200),
            "utm_term" => campaign.utm_term = clip(&v, 200),
            _ => {}
        }
        // 설정 순서대로 첫 번째 클릭 ID를 채택한다.
        if campaign.click_id.is_empty()
            && !v.is_empty()
            && settings.click_id_params.iter().any(|p| *p == *k)
        {
            campaign.click_id = clip(&v, 256);
            campaign.click_id_type = k.into_owned();
        }
    }

    if kept == 0 {
        url.set_query(None);
    } else if stripped {
        // 느린 경로: 남길 파라미터만 다시 직렬화한다 (제거가 있을 때만).
        let query = url.query().unwrap_or_default().to_owned();
        let mut out = url::form_urlencoded::Serializer::new(String::with_capacity(query.len()));
        for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
            if !settings.should_strip(&k) {
                out.append_pair(&k, &v);
            }
        }
        url.set_query(Some(&out.finish()));
    }
    url.set_fragment(None);

    let path = url.path().to_owned();
    Some(PageUrl {
        url: clip_owned(url.into(), crate::event::MAX_URL_LEN),
        host,
        path,
        campaign,
    })
}

/// referrer에서 host만 뽑는다 (쿼리는 개인정보가 섞일 수 있어 저장하지 않는다).
pub fn referrer_parts(raw: &str) -> Option<(String, String)> {
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "android-app") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let cleaned = format!("{}://{}{}", url.scheme(), host, url.path());
    Some((clip(&cleaned, crate::event::MAX_URL_LEN), host))
}

/// 2단계 공개 접미사 (eTLD+1 근사용). 한국 광고주 기준으로 자주 나오는 것 위주.
const TWO_LEVEL_SUFFIXES: &[&str] = &[
    "co.kr", "or.kr", "go.kr", "ne.kr", "ac.kr", "re.kr", "pe.kr", "ms.kr", "hs.kr", "es.kr",
    "kg.kr", "sc.kr", "co.jp", "ne.jp", "or.jp", "co.uk", "org.uk", "com.au", "com.cn", "com.tw",
    "com.hk", "com.sg", "co.nz",
];

/// 등록 가능 도메인(eTLD+1) 근사. `www.shop.client.co.kr` → `client.co.kr`.
pub fn registrable_domain(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 || host.parse::<std::net::IpAddr>().is_ok() {
        return host;
    }
    let last_two = labels[labels.len() - 2..].join(".");
    let take = if TWO_LEVEL_SUFFIXES.contains(&last_two.as_str()) {
        3
    } else {
        2
    };
    labels[labels.len().saturating_sub(take)..].join(".")
}

pub fn same_site(a: &str, b: &str) -> bool {
    registrable_domain(a) == registrable_domain(b)
}

/// 허용 도메인 매칭: `client.com`은 `client.com`과 모든 하위 도메인을 허용한다.
/// 요청마다 호출되므로 허용 항목마다 문자열을 만들지 않는다 (대소문자 무시 비교).
pub fn host_allowed(host: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|a| {
        let a = a.trim().trim_start_matches("*.");
        if host.len() == a.len() {
            return host.eq_ignore_ascii_case(a);
        }
        // `.{a}`로 끝나는지: 바이트 단위로 자르므로 경계가 문자 중간이어도 안전하다.
        host.len() > a.len()
            && host.as_bytes()[host.len() - a.len() - 1] == b'.'
            && host.as_bytes()[host.len() - a.len()..].eq_ignore_ascii_case(a.as_bytes())
    })
}

pub fn clip(s: &str, max: usize) -> String {
    clip_str(s, max).to_owned()
}

/// [`clip`]의 소유 버전: 새로 할당하지 않고 그 자리에서 자른다.
pub fn clip_owned(mut s: String, max: usize) -> String {
    let n = clip_str(&s, max).len();
    s.truncate(n);
    s
}

/// [`clip`]의 빌림 버전: 앞 `max`바이트 이내의 가장 긴 UTF-8 접두사.
pub fn clip_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_campaign_and_strips_pii() {
        let s = TrackingSettings::default();
        let p = parse_page_url(
            "https://www.lotteimall.com/coop/affilGate.lotte?chl_no=141&utm_source=naver&email=a@b.com&abx_tid=1794994866344:5c8a#disp",
            &s,
        )
        .unwrap();
        assert_eq!(p.host, "www.lotteimall.com");
        assert_eq!(p.path, "/coop/affilGate.lotte");
        assert_eq!(p.campaign.utm_source, "naver");
        assert_eq!(p.campaign.click_id_type, "abx_tid");
        assert_eq!(p.campaign.click_id, "1794994866344:5c8a");
        assert!(!p.url.contains("email"));
        assert!(!p.url.contains("#disp"));
        assert!(p.url.contains("chl_no=141"));
    }

    #[test]
    fn query_without_stripped_params_is_kept_verbatim() {
        // 제거할 파라미터가 없으면 쿼리 문자열을 다시 인코딩하지 않는다 (순서·인코딩 그대로)
        let s = TrackingSettings::default();
        let q = "b=2&a=1&utm_source=naver&q=%ED%95%9C%EA%B8%80&x=a+b&y=c%20d&empty=&flag&gclid=G1";
        let p = parse_page_url(&format!("https://Shop.Example.com/p?{q}#frag"), &s).unwrap();
        assert_eq!(p.url, format!("https://shop.example.com/p?{q}"));
        assert_eq!(p.host, "shop.example.com");
        assert_eq!(p.campaign.utm_source, "naver");
        assert_eq!(p.campaign.click_id_type, "gclid");
        assert_eq!(p.campaign.click_id, "G1");
    }

    #[test]
    fn stripped_params_removed_and_rest_kept_in_order() {
        let s = TrackingSettings {
            strip_query_params: vec!["Coupon".into()],
            ..Default::default()
        };
        let p = parse_page_url(
            "https://a.com/?z=1&EMAIL=x%40y.com&coupon=C&a=%ED%95%9C",
            &s,
        )
        .unwrap();
        assert_eq!(p.url, "https://a.com/?z=1&a=%ED%95%9C");
        // 모두 제거되면 `?`도 남기지 않는다
        let p = parse_page_url("https://a.com/x?email=a&phone=1", &s).unwrap();
        assert_eq!(p.url, "https://a.com/x");
        let p = parse_page_url("https://a.com/x?", &s).unwrap();
        assert_eq!(p.url, "https://a.com/x");
    }

    #[test]
    fn rejects_non_http() {
        assert!(parse_page_url("javascript:alert(1)", &TrackingSettings::default()).is_none());
    }

    #[test]
    fn registrable_domains() {
        assert_eq!(registrable_domain("www.shop.client.co.kr"), "client.co.kr");
        assert_eq!(registrable_domain("m.search.naver.com"), "naver.com");
        assert_eq!(registrable_domain("localhost"), "localhost");
        assert!(same_site("www.client.com", "track.client.com"));
        assert!(!same_site("client.com", "other.com"));
    }

    #[test]
    fn allowed_hosts() {
        let allowed = vec!["client.com".to_string()];
        assert!(host_allowed("client.com", &allowed));
        assert!(host_allowed("www.client.com", &allowed));
        assert!(!host_allowed("evilclient.com", &allowed));
        let allowed = vec![" *.Client.com ".to_string(), "b.org".to_string()];
        assert!(host_allowed("WWW.CLIENT.COM", &allowed));
        assert!(host_allowed("client.com", &allowed));
        assert!(host_allowed("b.org", &allowed));
        assert!(!host_allowed("xclient.com", &allowed));
        assert!(!host_allowed("client.com.evil.io", &allowed));
        assert!(!host_allowed("anything", &[]));
    }

    #[test]
    fn clip_respects_char_boundary() {
        assert_eq!(clip("가나다", 4), "가");
        assert_eq!(clip_owned("가나다".into(), 4), "가");
        assert_eq!(clip_owned("abc".into(), 3), "abc");
        assert_eq!(clip_owned("abcd".into(), 0), "");
        assert_eq!(clip_str("가나다", 6), "가나");
    }
}

//! 수집 배치 검증과 보강.
//!
//! Cloudflare Worker와 ingest의 엣지 에뮬레이션 모드가 같은 함수를 쓴다.
//! I/O는 하지 않는다. UUID 생성은 호출자가 주입한다.

use crate::channel::{self, ChannelInput, ChannelRule};
use crate::event::{self, Ecommerce, Event, Item, WireBatch, WireEvent, WireLanding, names};
use crate::settings::TrackingSettings;
use crate::urlx::{self, Campaign, clip};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// 클라이언트 시각을 믿을 수 있는 최대 오차.
const MAX_CLOCK_SKEW_MS: i64 = 24 * 60 * 60 * 1000;

/// Worker KV / ingest 캐시에 들어가는 프로젝트 수집 설정.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project_id: u32,
    pub public_key: String,
    pub active: bool,
    pub allowed_origins: Vec<String>,
    pub settings: TrackingSettings,
    pub settings_version: u32,
    pub channel_rules: Vec<ChannelRule>,
    /// 회원 해시 재해싱용 salt (hex)
    pub salt_hex: String,
}

/// 요청 맥락. 엣지(또는 에뮬레이터)가 헤더에서 채운다.
#[derive(Debug, Default, Clone)]
pub struct RequestCtx<'a> {
    pub origin: Option<&'a str>,
    pub referer: Option<&'a str>,
    pub user_agent: &'a str,
    pub country: &'a str,
    pub region: &'a str,
    pub city: &'a str,
    /// 서버 쿠키 `_at_vid` 값
    pub cookie_vid: Option<Uuid>,
    pub dnt: bool,
    pub now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Reject {
    #[error("프로젝트가 비활성 상태")]
    Inactive,
    #[error("허용되지 않은 출처")]
    OriginNotAllowed,
    #[error("봇 트래픽")]
    Bot,
    #[error("Do Not Track")]
    DoNotTrack,
    #[error("잘못된 배치: {0}")]
    Invalid(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Processed {
    pub visitor_id: Uuid,
    /// 서버 쿠키를 새로 발급해야 하는지
    pub issue_cookie: bool,
    pub events: Vec<Event>,
    /// 검증에 실패해 버린 이벤트 수
    pub dropped: usize,
}

/// 배치를 검증·보강해 저장할 이벤트 목록을 만든다.
pub fn process(
    ctx: &RequestCtx<'_>,
    cfg: &ProjectConfig,
    batch: WireBatch,
    mut new_uuid: impl FnMut() -> Uuid,
) -> Result<Processed, Reject> {
    if !cfg.active {
        return Err(Reject::Inactive);
    }
    if batch.e.is_empty() || batch.e.len() > event::MAX_EVENTS_PER_BATCH {
        return Err(Reject::Invalid("이벤트 수"));
    }
    // Referer는 Origin 헤더가 아예 없을 때만 쓴다. `Origin: null`처럼 값이 있지만 해석할 수 없으면
    // 거부한다 (sandbox iframe 등에서 Referer를 위조·유지한 요청이 허용 도메인으로 통과하지 않게).
    let origin_host = match ctx.origin {
        Some(o) => host_of(o),
        None => ctx.referer.and_then(host_of),
    }
    .ok_or(Reject::OriginNotAllowed)?;
    if !urlx::host_allowed(&origin_host, &cfg.allowed_origins) {
        return Err(Reject::OriginNotAllowed);
    }
    if cfg.settings.respect_dnt && ctx.dnt {
        return Err(Reject::DoNotTrack);
    }
    let ua = parse_ua(ctx.user_agent);
    if ua.is_bot {
        return Err(Reject::Bot);
    }

    let (visitor_id, issue_cookie) = match ctx.cookie_vid {
        Some(v) => (v, false),
        None => (
            batch
                .v
                .as_deref()
                .and_then(parse_uuid)
                .unwrap_or_else(&mut new_uuid),
            true,
        ),
    };
    let session_id = batch
        .s
        .as_deref()
        .and_then(parse_uuid)
        .unwrap_or_else(&mut new_uuid);
    let member_hash = batch
        .m
        .as_deref()
        .filter(|m| is_sha256_hex(m))
        .map(|m| rehash_member(&cfg.salt_hex, m))
        .unwrap_or_default();

    let session_source = batch.l.as_ref().and_then(|l| session_source(cfg, l));

    let mut events = Vec::with_capacity(batch.e.len());
    let mut dropped = 0;
    for we in batch.e {
        let ids = Ids {
            visitor_id,
            session_id,
            member_hash: &member_hash,
        };
        match build_event(
            ctx,
            cfg,
            &ua,
            &ids,
            session_source.as_ref(),
            we,
            &mut new_uuid,
        ) {
            Some(e) => events.push(e),
            None => dropped += 1,
        }
    }
    Ok(Processed {
        visitor_id,
        issue_cookie,
        events,
        dropped,
    })
}

struct Ids<'a> {
    visitor_id: Uuid,
    session_id: Uuid,
    member_hash: &'a str,
}

/// 세션 범위 유입 정보: 랜딩 페이지의 캠페인과 채널.
#[derive(Debug, Clone)]
struct SessionSource {
    campaign: Campaign,
    channel: String,
}

fn session_source(cfg: &ProjectConfig, l: &WireLanding) -> Option<SessionSource> {
    let page = urlx::parse_page_url(&l.u, &cfg.settings)?;
    if !urlx::host_allowed(&page.host, &cfg.allowed_origins) {
        return None;
    }
    let referrer_host =
        l.r.as_deref()
            .and_then(urlx::referrer_parts)
            .map(|(_, h)| h)
            .unwrap_or_default();
    let channel = classify_campaign(&page.campaign, &referrer_host, &page.host, cfg);
    Some(SessionSource {
        campaign: page.campaign,
        channel,
    })
}

fn classify_campaign(
    c: &Campaign,
    referrer_host: &str,
    page_host: &str,
    cfg: &ProjectConfig,
) -> String {
    channel::classify(
        &ChannelInput {
            utm_source: &c.utm_source,
            utm_medium: &c.utm_medium,
            utm_campaign: &c.utm_campaign,
            click_id_type: &c.click_id_type,
            referrer_host,
            page_host,
        },
        &cfg.channel_rules,
    )
}

fn build_event(
    ctx: &RequestCtx<'_>,
    cfg: &ProjectConfig,
    ua: &Ua,
    ids: &Ids<'_>,
    session: Option<&SessionSource>,
    we: WireEvent,
    new_uuid: &mut impl FnMut() -> Uuid,
) -> Option<Event> {
    let s = &cfg.settings;
    let name = we.n.as_str();
    if !event::is_valid_event_name(name) {
        return None;
    }
    let allowed = match name {
        names::OUTBOUND_CLICK => s.outbound_links,
        names::FILE_DOWNLOAD => s.file_downloads,
        names::SCROLL => s.scroll_depth,
        n if names::is_ecommerce(n) => s.ecommerce,
        _ => true,
    };
    if !allowed {
        return None;
    }

    let page = urlx::parse_page_url(&we.u, s)?;
    if !urlx::host_allowed(&page.host, &cfg.allowed_origins) {
        return None;
    }
    let (referrer, referrer_host) =
        we.r.as_deref()
            .filter(|r| !r.is_empty())
            .and_then(urlx::referrer_parts)
            .unwrap_or_default();

    let is_session_start = we.ss.unwrap_or(false);
    // 세션 시작 이벤트는 자기 URL·referrer로, 나머지는 세션 랜딩 정보로 채널을 정한다.
    let (campaign, channel) = match session {
        Some(src) if !is_session_start => (src.campaign.clone(), src.channel.clone()),
        _ => {
            let ch = classify_campaign(&page.campaign, &referrer_host, &page.host, cfg);
            (page.campaign.clone(), ch)
        }
    };

    let ts_ms = if (we.t - ctx.now_ms).abs() > MAX_CLOCK_SKEW_MS {
        ctx.now_ms
    } else {
        we.t
    };
    let ec = we.ec.map(|ec| Ecommerce {
        order_id: clip(ec.order_id.as_deref().unwrap_or(""), 128),
        value: money(ec.value.unwrap_or(0.0)),
        currency: clip(
            &ec.currency
                .unwrap_or_else(|| "KRW".into())
                .to_ascii_uppercase(),
            3,
        ),
        items: ec
            .items
            .into_iter()
            .take(event::MAX_ITEMS)
            .map(|i| Item {
                id: clip(&i.id, 128),
                name: clip(&i.name, 256),
                price: money(i.price),
                qty: i.qty,
            })
            .collect(),
    });

    Some(Event {
        project_id: cfg.project_id,
        event_id: we
            .id
            .as_deref()
            .and_then(parse_uuid)
            .unwrap_or_else(&mut *new_uuid),
        ts_ms,
        received_at_ms: ctx.now_ms,
        event_name: name.to_owned(),
        visitor_id: ids.visitor_id,
        session_id: ids.session_id,
        is_session_start,
        member_hash: ids.member_hash.to_owned(),
        url: page.url,
        host: page.host,
        path: clip(&page.path, 1024),
        title: clip(we.ti.as_deref().unwrap_or(""), 300),
        referrer,
        referrer_host,
        utm_source: campaign.utm_source,
        utm_medium: campaign.utm_medium,
        utm_campaign: campaign.utm_campaign,
        utm_content: campaign.utm_content,
        utm_term: campaign.utm_term,
        click_id: campaign.click_id,
        click_id_type: campaign.click_id_type,
        channel,
        country: clip(ctx.country, 2),
        region: clip(ctx.region, 64),
        city: clip(ctx.city, 64),
        device_type: ua.device_type.to_owned(),
        browser: ua.browser.clone(),
        os: ua.os.clone(),
        screen_w: we.sw.unwrap_or(0).min(u16::MAX as u32) as u16,
        props: props(we.p),
        ec,
    })
}

fn props(
    p: Option<std::collections::BTreeMap<String, serde_json::Value>>,
) -> Vec<(String, String)> {
    let Some(p) = p else { return vec![] };
    p.into_iter()
        .filter(|(k, _)| !k.is_empty() && k.len() <= event::MAX_PROP_KEY_LEN)
        .take(event::MAX_PROPS)
        .filter_map(|(k, v)| {
            let v = match v {
                serde_json::Value::Null => return None,
                serde_json::Value::String(s) => s,
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            Some((k, clip(&v, event::MAX_PROP_VALUE_LEN)))
        })
        .collect()
}

/// 금액을 통화 최소단위 정수로 (KRW: 원). 음수·NaN은 0.
fn money(v: f64) -> i64 {
    if v.is_finite() && v > 0.0 {
        v.round().min(i64::MAX as f64) as i64
    } else {
        0
    }
}

fn host_of(s: &str) -> Option<String> {
    url::Url::parse(s)
        .ok()?
        .host_str()
        .map(|h| h.to_ascii_lowercase())
}

fn parse_uuid(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s).ok().filter(|u| !u.is_nil())
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 프로젝트 salt로 회원 해시를 다시 해싱한다. 프로젝트끼리 같은 회원을 연결할 수 없게 하기 위해서다.
pub fn rehash_member(salt_hex: &str, member_sha256_hex: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt_hex.as_bytes());
    h.update(b":");
    h.update(member_sha256_hex.to_ascii_lowercase().as_bytes());
    hex::encode(h.finalize())
}

struct Ua {
    is_bot: bool,
    device_type: &'static str,
    browser: String,
    os: String,
}

const BOT_MARKERS: &[&str] = &[
    "bot",
    "spider",
    "crawl",
    "headless",
    "lighthouse",
    "pingdom",
    "uptime",
    "monitor",
    "curl/",
    "wget",
    "python-requests",
    "go-http-client",
    "axios/",
    "node-fetch",
    "phantomjs",
    "selenium",
    "puppeteer",
    "playwright",
    "yeti",
    "daum",
];

fn parse_ua(ua: &str) -> Ua {
    let lower = ua.to_ascii_lowercase();
    if ua.trim().is_empty() || BOT_MARKERS.iter().any(|m| lower.contains(m)) {
        return Ua {
            is_bot: true,
            device_type: "",
            browser: String::new(),
            os: String::new(),
        };
    }
    let parsed = woothee::parser::Parser::new().parse(ua);
    let (category, browser, os) = parsed
        .map(|r| (r.category.to_owned(), r.name.to_owned(), r.os.to_owned()))
        .unwrap_or_default();
    let is_bot = category == "crawler";
    let device_type =
        if lower.contains("ipad") || (lower.contains("android") && !lower.contains("mobile")) {
            "tablet"
        } else {
            match category.as_str() {
                "smartphone" | "mobilephone" => "mobile",
                "pc" => "desktop",
                "appliance" => "other",
                _ => "unknown",
            }
        };
    Ua {
        is_bot,
        device_type,
        browser,
        os,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{WireEcommerce, WireItem};

    const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";

    fn cfg() -> ProjectConfig {
        ProjectConfig {
            project_id: 7,
            public_key: "pk_testtest01".into(),
            active: true,
            allowed_origins: vec!["client.com".into()],
            settings: TrackingSettings::default(),
            settings_version: 1,
            channel_rules: vec![],
            salt_hex: "00ff".into(),
        }
    }

    fn ctx(ua: &str) -> RequestCtx<'_> {
        RequestCtx {
            origin: Some("https://www.client.com"),
            user_agent: ua,
            country: "KR",
            now_ms: 1_763_631_912_000,
            ..Default::default()
        }
    }

    fn ev(name: &str, url: &str) -> WireEvent {
        WireEvent {
            id: None,
            n: name.into(),
            t: 1_763_631_912_000,
            u: url.into(),
            r: None,
            ti: None,
            sw: Some(390),
            ss: Some(true),
            p: None,
            ec: None,
        }
    }

    fn batch(e: Vec<WireEvent>) -> WireBatch {
        WireBatch {
            k: "pk_testtest01".into(),
            v: None,
            s: None,
            m: None,
            sv: 1,
            l: None,
            e,
        }
    }

    fn counter() -> impl FnMut() -> Uuid {
        let mut n = 0u128;
        move || {
            n += 1;
            Uuid::from_u128(n)
        }
    }

    #[test]
    fn enriches_pageview() {
        let mut e = ev(
            "page_view",
            "https://www.client.com/p/1?utm_source=Naver&utm_medium=cpc&n_keyword=x",
        );
        e.r = Some("https://search.naver.com/search.naver?query=패딩".into());
        let out = process(&ctx(IPHONE), &cfg(), batch(vec![e]), counter()).unwrap();
        assert!(out.issue_cookie);
        let ev = &out.events[0];
        assert_eq!(ev.channel, "naver_sa");
        assert_eq!(ev.referrer, "https://search.naver.com/search.naver");
        assert_eq!(ev.referrer_host, "search.naver.com");
        assert_eq!(ev.device_type, "mobile");
        assert_eq!(ev.country, "KR");
        assert!(ev.is_session_start);
        assert_eq!(ev.screen_w, 390);
    }

    #[test]
    fn server_cookie_wins_over_candidate() {
        let mut c = ctx(CHROME);
        let vid = Uuid::from_u128(999);
        c.cookie_vid = Some(vid);
        let mut b = batch(vec![ev("page_view", "https://client.com/")]);
        b.v = Some(Uuid::from_u128(5).to_string());
        let out = process(&c, &cfg(), b, counter()).unwrap();
        assert_eq!(out.visitor_id, vid);
        assert!(!out.issue_cookie);
    }

    #[test]
    fn candidate_vid_adopted_without_cookie() {
        let mut b = batch(vec![ev("page_view", "https://client.com/")]);
        let cand = Uuid::from_u128(5);
        b.v = Some(cand.to_string());
        let out = process(&ctx(CHROME), &cfg(), b, counter()).unwrap();
        assert_eq!(out.visitor_id, cand);
    }

    #[test]
    fn rejects_foreign_origin_and_bots() {
        let mut c = ctx(CHROME);
        c.origin = Some("https://evil.com");
        let b = batch(vec![ev("page_view", "https://client.com/")]);
        assert_eq!(
            process(&c, &cfg(), b.clone(), counter()),
            Err(Reject::OriginNotAllowed)
        );
        assert_eq!(
            process(&ctx("Googlebot/2.1"), &cfg(), b.clone(), counter()),
            Err(Reject::Bot)
        );
        assert_eq!(
            process(&ctx("HeadlessChrome/120"), &cfg(), b, counter()),
            Err(Reject::Bot)
        );
    }

    #[test]
    fn present_but_invalid_origin_does_not_fall_back_to_referer() {
        // `Origin: null`(sandbox iframe, file:// 등)이나 해석할 수 없는 Origin은 거부한다.
        // Referer로 대신하는 것은 Origin 헤더가 아예 없을 때뿐이다.
        let b = batch(vec![ev("page_view", "https://client.com/")]);
        for bad in ["null", "", "not a url", "https://"] {
            let mut c = ctx(CHROME);
            c.origin = Some(bad);
            c.referer = Some("https://www.client.com/p/1");
            assert_eq!(
                process(&c, &cfg(), b.clone(), counter()),
                Err(Reject::OriginNotAllowed),
                "{bad:?}"
            );
        }
        let mut c = ctx(CHROME);
        c.origin = None;
        c.referer = Some("https://www.client.com/p/1");
        assert!(
            process(&c, &cfg(), b, counter()).is_ok(),
            "Origin이 없으면 Referer"
        );
    }

    #[test]
    fn drops_disabled_and_invalid_events() {
        let b = batch(vec![
            ev("page_view", "https://client.com/"),
            ev("add_to_cart", "https://client.com/"), // ecommerce off
            ev("scroll", "https://client.com/"),      // scroll off
            ev("Bad Name", "https://client.com/"),    // invalid
            ev("page_view", "https://other.com/"),    // page host not allowed
            ev("signup", "https://client.com/"),
        ]);
        let out = process(&ctx(CHROME), &cfg(), b, counter()).unwrap();
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.dropped, 4);
    }

    #[test]
    fn ecommerce_and_props() {
        let mut c = cfg();
        c.settings.ecommerce = true;
        let mut e = ev("purchase", "https://client.com/done");
        e.ec = Some(WireEcommerce {
            order_id: Some("ORD-9901".into()),
            value: Some(100000.4),
            currency: None,
            items: vec![WireItem {
                id: "SKU1".into(),
                name: "패딩".into(),
                price: 100000.0,
                qty: 1,
            }],
        });
        e.p = Some(
            [
                ("plan".to_string(), serde_json::json!("pro")),
                ("n".into(), serde_json::json!(3)),
                ("x".into(), serde_json::Value::Null),
            ]
            .into_iter()
            .collect(),
        );
        let out = process(&ctx(CHROME), &c, batch(vec![e]), counter()).unwrap();
        let ev = &out.events[0];
        let ec = ev.ec.as_ref().unwrap();
        assert_eq!(ec.value, 100000);
        assert_eq!(ec.currency, "KRW");
        assert_eq!(ec.items[0].price, 100000);
        assert_eq!(
            ev.props,
            vec![
                ("n".to_string(), "3".to_string()),
                ("plan".into(), "pro".into())
            ]
        );
    }

    #[test]
    fn events_inherit_session_channel() {
        let mut landing = ev(
            "page_view",
            "https://client.com/?utm_source=naver&utm_medium=cpc",
        );
        landing.ss = Some(true);
        let mut spa = ev("page_view", "https://client.com/product/1");
        spa.ss = Some(false);
        spa.r = Some("https://client.com/?utm_source=naver".into());
        let mut buy = ev("signup", "https://client.com/product/1");
        buy.ss = Some(false);
        let mut b = batch(vec![landing, spa, buy]);
        b.l = Some(WireLanding {
            u: "https://client.com/?utm_source=naver&utm_medium=cpc".into(),
            r: None,
        });
        let out = process(&ctx(CHROME), &cfg(), b, counter()).unwrap();
        let chans: Vec<_> = out.events.iter().map(|e| e.channel.as_str()).collect();
        assert_eq!(chans, vec!["naver_sa", "naver_sa", "naver_sa"]);
        assert!(out.events.iter().all(|e| e.utm_source == "naver"));
        // 페이지 referrer는 이벤트별 값을 유지한다.
        assert_eq!(out.events[1].referrer_host, "client.com");
    }

    #[test]
    fn landing_on_foreign_host_is_ignored() {
        let mut e = ev("signup", "https://client.com/");
        e.ss = Some(false);
        let mut b = batch(vec![e]);
        b.l = Some(WireLanding {
            u: "https://evil.com/?utm_source=x".into(),
            r: None,
        });
        let out = process(&ctx(CHROME), &cfg(), b, counter()).unwrap();
        assert_eq!(out.events[0].channel, "direct");
    }

    #[test]
    fn clock_skew_is_corrected() {
        let mut e = ev("page_view", "https://client.com/");
        e.t = 0;
        let out = process(&ctx(CHROME), &cfg(), batch(vec![e]), counter()).unwrap();
        assert_eq!(out.events[0].ts_ms, 1_763_631_912_000);
    }

    #[test]
    fn member_hash_is_rehashed_with_salt() {
        let mut b = batch(vec![ev("page_view", "https://client.com/")]);
        let raw = "8d969eef6ecad3c29a3a629280e686cf0c3f5d5a86aff3ca12020c923adc6c92";
        b.m = Some(raw.into());
        let out = process(&ctx(CHROME), &cfg(), b, counter()).unwrap();
        let h = &out.events[0].member_hash;
        assert_eq!(h.len(), 64);
        assert_ne!(h, raw);
        assert_eq!(h, &rehash_member("00ff", raw));
    }
}

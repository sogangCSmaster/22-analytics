//! `POST /v1/collect` 판단 로직 (ARCHITECTURE §4.1). ingest `emulate.rs`와 같은 동작을 하되,
//! 거부 사유는 응답에 남기지 않는다 (항상 `204`, 본문 크기 초과만 `413`).

use crate::cookie;
use crate::cors::{self, OriginCheck};
use crate::http::EdgeResponse;
use analytics_core::collect::{self, ProjectConfig, RequestCtx};
use analytics_core::event::{MAX_BATCH_BYTES, MAX_URL_LEN, WireBatch};
use analytics_core::urlx::clip;
use analytics_core::{Event, PublicKey};
use uuid::Uuid;

/// UA 파서(woothee)에 넘기는 최대 길이. 비정상적으로 긴 UA로 CPU를 쓰게 하는 것을 막는다.
pub const MAX_UA_LEN: usize = 1024;
/// Referer는 URL 한도의 두 배까지만 본다. 넘으면 없는 것으로 친다.
pub const MAX_REFERER_LEN: usize = MAX_URL_LEN * 2;

/// 요청에서 뽑은 값 (wasm 계층이 헤더와 `request.cf`로 채운다).
#[derive(Debug, Clone, Default)]
pub struct CollectRequest<'a> {
    /// 요청 URL의 호스트 (커스텀 호스트명). 쿠키 Domain 계산용.
    pub host: &'a str,
    pub origin: Option<&'a str>,
    pub referer: Option<&'a str>,
    pub user_agent: &'a str,
    pub cookie: Option<&'a str>,
    pub dnt: bool,
    pub country: &'a str,
    pub region: &'a str,
    pub city: &'a str,
    pub now_ms: i64,
}

/// 본문을 배치로 해석한다. 크기 초과는 `413`, 그 밖의 오류는 사유 없는 `204`.
pub fn parse_body(body: &[u8]) -> Result<(PublicKey, WireBatch), EdgeResponse> {
    if body.len() > MAX_BATCH_BYTES {
        return Err(EdgeResponse::empty(413));
    }
    let silent = || EdgeResponse::empty(204);
    let batch: WireBatch = serde_json::from_slice(body).map_err(|_| silent())?;
    let key = PublicKey::parse(&batch.k).map_err(|_| silent())?;
    Ok((key, batch))
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollectOutcome {
    pub response: EdgeResponse,
    /// 원점으로 보낼 이벤트 (비어 있으면 전달하지 않는다)
    pub events: Vec<Event>,
    /// 버린 이벤트 수 (로그용)
    pub dropped: usize,
}

/// 프로젝트 설정(`None` = 없는 키)과 배치로 응답과 전달할 이벤트를 정한다.
pub fn handle(
    req: &CollectRequest<'_>,
    cfg: Option<&ProjectConfig>,
    batch: WireBatch,
    new_uuid: impl FnMut() -> Uuid,
) -> CollectOutcome {
    let silent = || CollectOutcome {
        response: EdgeResponse::empty(204),
        events: vec![],
        dropped: 0,
    };
    let Some(cfg) = cfg.filter(|c| c.public_key == batch.k) else {
        return silent();
    };
    // Origin이 있으면 Origin만 본다: `null`이나 허용 목록 밖이면 Referer로 우회하지 못한다.
    let (origin, mut response) = match cors::check_origin(req.origin, &cfg.allowed_origins) {
        OriginCheck::Denied => return silent(),
        OriginCheck::Absent => (None, EdgeResponse::empty(204)),
        OriginCheck::Allowed(o) => {
            let mut r = EdgeResponse::empty(204);
            r.headers = cors::credentialed_headers(&o);
            (Some(o), r)
        }
    };

    let user_agent = clip(req.user_agent, MAX_UA_LEN);
    let ctx = RequestCtx {
        origin: origin.as_deref(),
        referer: req.referer.filter(|r| r.len() <= MAX_REFERER_LEN),
        user_agent: &user_agent,
        country: req.country,
        region: req.region,
        city: req.city,
        cookie_vid: cookie::visitor_id(req.cookie),
        dnt: req.dnt,
        now_ms: req.now_ms,
    };
    match collect::process(&ctx, cfg, batch, new_uuid) {
        Ok(p) => {
            if p.issue_cookie {
                response.headers.push((
                    "set-cookie".into(),
                    cookie::set_cookie(p.visitor_id, req.host),
                ));
            }
            CollectOutcome {
                response,
                events: p.events,
                dropped: p.dropped,
            }
        }
        Err(_) => CollectOutcome {
            response,
            events: vec![],
            dropped: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::project;
    use crate::cookie::VISITOR_COOKIE;
    use crate::forward::{self, tests::SECRET};
    use analytics_core::edge_auth::{HEADER_SIGNATURE, HEADER_TIMESTAMP, verify};

    const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    const NOW: i64 = 1_763_631_912_000;
    const VID: &str = "5c8a0d27-0861-4c76-b062-bc938f8c6833";

    fn req() -> CollectRequest<'static> {
        CollectRequest {
            host: "track.client.com",
            origin: Some("https://www.client.com"),
            referer: Some("https://www.client.com/p/1"),
            user_agent: CHROME,
            cookie: None,
            dnt: false,
            country: "KR",
            region: "Seoul",
            city: "Seoul",
            now_ms: NOW,
        }
    }

    fn body(events: &str) -> Vec<u8> {
        format!(
            r#"{{"k":"pk_testtest01","s":"11111111-2222-4333-8444-555555555555","e":[{events}]}}"#
        )
        .into_bytes()
    }

    fn pv(id: &str) -> String {
        format!(
            r#"{{"id":"{id}","n":"page_view","t":{NOW},"u":"https://www.client.com/p/1?utm_source=naver&utm_medium=cpc","ss":true}}"#
        )
    }

    const E1: &str = "aaaaaaaa-0000-4000-8000-000000000001";
    const E2: &str = "aaaaaaaa-0000-4000-8000-000000000002";

    fn counter() -> impl FnMut() -> Uuid {
        let mut n = 0u128;
        move || {
            n += 1;
            Uuid::from_u128(0xFEED_0000 + n)
        }
    }

    fn run(r: &CollectRequest<'_>, cfg: Option<&ProjectConfig>, b: &[u8]) -> CollectOutcome {
        let (_, batch) = parse_body(b).expect("parse");
        handle(r, cfg, batch, counter())
    }

    fn assert_silent_204(o: &CollectOutcome) {
        assert_eq!(o.response.status, 204);
        assert!(o.events.is_empty());
        assert!(o.response.body.is_empty());
        assert_eq!(o.response.header("set-cookie"), None);
        assert_eq!(
            o.response.header("x-at-status"),
            None,
            "사유를 노출하지 않는다"
        );
    }

    // ── 해피 ──
    #[test]
    fn valid_batch_produces_signed_forward() {
        let p = project();
        let o = run(&req(), Some(&p), &body(&format!("{},{}", pv(E1), pv(E2))));
        assert_eq!(o.response.status, 204);
        assert_eq!(o.events.len(), 2);
        let e = &o.events[0];
        assert_eq!(e.project_id, 7);
        assert_eq!(e.event_id, Uuid::parse_str(E1).unwrap());
        assert_eq!(e.country, "KR");
        assert_eq!(e.region, "Seoul");
        assert_eq!(e.city, "Seoul");
        assert_eq!(e.channel, "naver_sa");
        assert_eq!(e.received_at_ms, NOW);

        // CORS: 허용 Origin을 credentials와 함께 되돌린다
        assert_eq!(
            o.response.header("access-control-allow-origin"),
            Some("https://www.client.com")
        );
        assert_eq!(
            o.response.header("access-control-allow-credentials"),
            Some("true")
        );

        // 쿠키가 없었으므로 새로 발급 (등록 가능 도메인)
        let sc = o.response.header("set-cookie").unwrap();
        assert!(
            sc.starts_with(&format!("{VISITOR_COOKIE}={}", e.visitor_id)),
            "{sc}"
        );
        assert!(sc.contains("Domain=client.com"));
        assert!(sc.contains("HttpOnly; Secure; SameSite=Lax"));

        // 원점 전달 요청은 core edge_auth로 검증된다
        let fwd = forward::signed(
            "https://origin",
            forward::COLLECT_PATH,
            SECRET,
            forward::collect_body(&o.events).into_bytes(),
            NOW,
        );
        let ts: i64 = fwd.header(HEADER_TIMESTAMP).unwrap().parse().unwrap();
        assert!(verify(
            SECRET,
            ts,
            &fwd.body,
            fwd.header(HEADER_SIGNATURE).unwrap(),
            NOW
        ));
    }

    #[test]
    fn existing_cookie_is_kept_and_not_reissued() {
        let p = project();
        let cookie = format!("other=1; {VISITOR_COOKIE}={VID}");
        let r = CollectRequest {
            cookie: Some(&cookie),
            ..req()
        };
        let o = run(&r, Some(&p), &body(&pv(E1)));
        assert_eq!(o.events[0].visitor_id, Uuid::parse_str(VID).unwrap());
        assert_eq!(o.response.header("set-cookie"), None);
    }

    #[test]
    fn missing_origin_falls_back_to_referer() {
        let p = project();
        let r = CollectRequest {
            origin: None,
            ..req()
        };
        let o = run(&r, Some(&p), &body(&pv(E1)));
        assert_eq!(o.events.len(), 1);
        assert_eq!(o.response.header("access-control-allow-origin"), None);
    }

    // ── 멱등성 ──
    #[test]
    fn duplicate_event_ids_pass_through_unchanged() {
        let p = project();
        let o = run(&req(), Some(&p), &body(&format!("{},{}", pv(E1), pv(E1))));
        assert_eq!(
            o.events.len(),
            2,
            "중복 제거는 하류(ClickHouse)가 event_id로 한다"
        );
        assert_eq!(o.events[0].event_id, o.events[1].event_id);
    }

    #[test]
    fn same_request_twice_same_events() {
        let p = project();
        let cookie = format!("{VISITOR_COOKIE}={VID}");
        let r = CollectRequest {
            cookie: Some(&cookie),
            ..req()
        };
        let a = run(&r, Some(&p), &body(&pv(E1)));
        let b = run(&r, Some(&p), &body(&pv(E1)));
        assert_eq!(a, b);
    }

    // ── 경계값·엣지 ──
    #[test]
    fn body_size_boundary() {
        // 정확히 64KB는 통과(해석 단계로), 1바이트 넘으면 413
        let mut exact = body(&pv(E1));
        let pad = MAX_BATCH_BYTES - exact.len();
        // JSON 끝 `}` 앞에 공백을 채워 유효한 JSON 유지
        let close = exact.pop().unwrap();
        exact.extend(std::iter::repeat_n(b' ', pad));
        exact.push(close);
        assert_eq!(exact.len(), MAX_BATCH_BYTES);
        assert!(parse_body(&exact).is_ok());

        let mut over = exact.clone();
        over.insert(over.len() - 1, b' ');
        assert_eq!(over.len(), MAX_BATCH_BYTES + 1);
        let r = parse_body(&over).unwrap_err();
        assert_eq!(r.status, 413);
        assert!(r.body.is_empty());
    }

    #[test]
    fn empty_and_malformed_bodies_are_silent_204() {
        for b in [
            &b""[..],
            b"   ",
            b"null",
            b"{}",
            b"[]",
            b"{\"k\":\"pk_testtest01\"",
            b"\xff\xfe",
            br#"{"k":"sk_live_abcdefgh","e":[]}"#,
            br#"{"k":"pk_x","e":[]}"#,
        ] {
            let r = parse_body(b).unwrap_err();
            assert_eq!(r.status, 204, "{:?}", String::from_utf8_lossy(b));
            assert!(r.headers.is_empty());
        }
    }

    #[test]
    fn empty_event_list_is_silent() {
        let p = project();
        let o = run(&req(), Some(&p), &body(""));
        assert_silent_204(&o);
    }

    #[test]
    fn workers_dev_host_omits_cookie_domain() {
        let p = project();
        let r = CollectRequest {
            host: "edge.agency.workers.dev",
            ..req()
        };
        let o = run(&r, Some(&p), &body(&pv(E1)));
        let sc = o.response.header("set-cookie").unwrap();
        assert!(!sc.contains("Domain="), "{sc}");
    }

    #[test]
    fn co_kr_host_cookie_domain() {
        let mut p = project();
        p.allowed_origins = vec!["shop.co.kr".into()];
        let r = CollectRequest {
            host: "track.shop.co.kr",
            origin: Some("https://www.shop.co.kr"),
            ..req()
        };
        let b = format!(
            r#"{{"k":"pk_testtest01","e":[{{"n":"page_view","t":{NOW},"u":"https://www.shop.co.kr/"}}]}}"#
        );
        let o = run(&r, Some(&p), b.as_bytes());
        assert_eq!(o.events.len(), 1);
        assert!(
            o.response
                .header("set-cookie")
                .unwrap()
                .contains("Domain=shop.co.kr;")
        );
    }

    #[test]
    fn huge_user_agent_is_clipped_not_fatal() {
        let p = project();
        let ua = format!("{CHROME} {}", "x".repeat(64 * 1024));
        let r = CollectRequest {
            user_agent: &ua,
            ..req()
        };
        let o = run(&r, Some(&p), &body(&pv(E1)));
        assert_eq!(o.events.len(), 1);
        assert_eq!(o.events[0].browser, "Chrome");
    }

    #[test]
    fn oversized_referer_is_ignored() {
        let p = project();
        let referer = format!("https://www.client.com/{}", "a".repeat(MAX_REFERER_LEN));
        let r = CollectRequest {
            origin: None,
            referer: Some(&referer),
            ..req()
        };
        // Origin도 없고 Referer도 무시되면 출처를 확인할 수 없어 버린다
        assert_silent_204(&run(&r, Some(&p), &body(&pv(E1))));
    }

    // ── 희귀 ──
    #[test]
    fn unknown_or_inactive_project_is_silent() {
        assert_silent_204(&run(&req(), None, &body(&pv(E1))));
        let mut p = project();
        p.active = false;
        let o = run(&req(), Some(&p), &body(&pv(E1)));
        assert_silent_204(&o);
    }

    #[test]
    fn bot_and_dnt_are_silent() {
        let p = project();
        let r = CollectRequest {
            user_agent: "Googlebot/2.1 (+http://www.google.com/bot.html)",
            ..req()
        };
        let o = run(&r, Some(&p), &body(&pv(E1)));
        assert!(o.events.is_empty());
        assert_eq!(o.response.status, 204);
        assert_eq!(o.response.header("set-cookie"), None);

        let mut p = project();
        p.settings.respect_dnt = true;
        let r = CollectRequest { dnt: true, ..req() };
        assert!(run(&r, Some(&p), &body(&pv(E1))).events.is_empty());
    }

    #[test]
    fn all_events_dropped_means_no_forward_but_cookie_ok() {
        let p = project();
        let b = body(&format!(
            r#"{{"n":"Bad Name","t":{NOW},"u":"https://www.client.com/"}}"#
        ));
        let o = run(&req(), Some(&p), &b);
        assert!(o.events.is_empty());
        assert_eq!(o.dropped, 1);
        assert_eq!(o.response.status, 204);
    }

    // ── 외부 공격 ──
    #[test]
    fn forged_origin_is_not_processed_and_gets_no_cors() {
        let p = project();
        for origin in ["https://evil.com", "https://client.com.evil.com", "null"] {
            let r = CollectRequest {
                origin: Some(origin),
                // Referer가 허용 도메인이어도 Origin이 있으면 Origin이 우선한다
                referer: Some("https://www.client.com/"),
                ..req()
            };
            let o = run(&r, Some(&p), &body(&pv(E1)));
            assert_silent_204(&o);
            assert_eq!(
                o.response.header("access-control-allow-origin"),
                None,
                "{origin}"
            );
        }
    }

    #[test]
    fn cookie_injection_never_reflected() {
        let p = project();
        let evil = format!(
            "{VISITOR_COOKIE}=x\r\nSet-Cookie: admin=1; {VISITOR_COOKIE}=; Domain=evil.com"
        );
        let r = CollectRequest {
            cookie: Some(&evil),
            ..req()
        };
        let o = run(&r, Some(&p), &body(&pv(E1)));
        let sc = o.response.header("set-cookie").unwrap();
        assert!(!sc.contains('\r') && !sc.contains('\n'));
        assert!(!sc.contains("evil") && !sc.contains("admin"));
        assert!(Uuid::parse_str(sc.split(['=', ';']).nth(1).unwrap()).is_ok());
    }

    #[test]
    fn candidate_vid_injection_is_replaced() {
        let p = project();
        let b = format!(
            r#"{{"k":"pk_testtest01","v":"x\r\nSet-Cookie: a=1","e":[{}]}}"#,
            pv(E1)
        );
        let o = run(&req(), Some(&p), b.as_bytes());
        let sc = o.response.header("set-cookie").unwrap();
        assert!(!sc.contains("Set-Cookie: a"));
        assert_eq!(o.events[0].visitor_id, Uuid::from_u128(0xFEED_0001));
    }

    #[test]
    fn key_mismatch_between_body_and_config_is_silent() {
        // KV가 다른 프로젝트 설정을 돌려준 경우(키 불일치)에도 섞이지 않게 한다
        let mut p = project();
        p.public_key = "pk_someoneelse".into();
        assert_silent_204(&run(&req(), Some(&p), &body(&pv(E1))));
    }
}

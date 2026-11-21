//! 서버 방문자 쿠키 `_at_vid` (ARCHITECTURE §3.4 모드 A).
//!
//! 쿠키 값으로는 항상 `Uuid`만 쓴다. 클라이언트가 보낸 문자열을 그대로 `Set-Cookie`에
//! 반사하지 않으므로 CRLF·세미콜론 주입이 구조적으로 불가능하다.

use analytics_core::urlx::registrable_domain;
use uuid::Uuid;

pub const VISITOR_COOKIE: &str = "_at_vid";
/// 1년
pub const COOKIE_MAX_AGE_SECS: u32 = 31_536_000;

/// 브라우저가 `Domain`을 거부하는 공개 호스팅 접미사 (PSL 등재). 이 아래에서는 Domain을 생략한다.
const HOSTED_SUFFIXES: &[&str] = &["workers.dev", "pages.dev"];

/// `Cookie` 헤더에서 `_at_vid`를 찾는다.
///
/// - 이름은 대소문자를 구분해 정확히 일치해야 한다.
/// - 같은 이름이 여러 번 오면 (경로가 더 구체적인 쿠키가 앞에 온다, RFC 6265 §5.4) 처음으로
///   UUID로 해석되는 값을 쓴다.
/// - `=`가 없는 조각, 빈 값, UUID가 아닌 값, nil UUID는 무시한다.
pub fn visitor_id(cookie_header: Option<&str>) -> Option<Uuid> {
    cookie_header?.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        if name.trim() != VISITOR_COOKIE {
            return None;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        Uuid::parse_str(value).ok().filter(|u| !u.is_nil())
    })
}

/// `Set-Cookie`의 `Domain` 값: 요청 호스트의 등록 가능 도메인.
///
/// IP 주소, `localhost`, 점 없는 호스트, `*.workers.dev`처럼 공개 접미사 아래 호스트,
/// 잘못된 문자가 섞인 호스트는 `None`(호스트 전용 쿠키)이다.
pub fn cookie_domain(host: &str) -> Option<String> {
    // IPv6 리터럴 (`[::1]`, `[::1]:8787`, 대괄호 없는 `::1`)
    if host.starts_with('[') || host.matches(':').count() > 1 {
        return None;
    }
    let host = match host.split_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        Some(_) => return None,
        None => host,
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let valid = !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && host.split('.').all(|l| !l.is_empty());
    if !valid || !host.contains('.') || host.parse::<std::net::Ipv4Addr>().is_ok() {
        return None;
    }
    // 마지막 라벨이 숫자뿐이면 IP 표기의 변형으로 본다
    if host.rsplit('.').next()?.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if HOSTED_SUFFIXES
        .iter()
        .any(|s| host == *s || host.ends_with(&format!(".{s}")))
    {
        return None;
    }
    let reg = registrable_domain(&host);
    // `co.kr`처럼 공개 접미사 자체면 registrable_domain("x.co.kr")이 자신과 다르다
    if registrable_domain(&format!("x.{reg}")) != reg {
        return None;
    }
    Some(reg)
}

/// `_at_vid` 발급용 `Set-Cookie` 헤더 값.
pub fn set_cookie(vid: Uuid, host: &str) -> String {
    let domain = cookie_domain(host)
        .map(|d| format!("; Domain={d}"))
        .unwrap_or_default();
    format!(
        "{VISITOR_COOKIE}={vid}{domain}; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}; HttpOnly; Secure; SameSite=Lax"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const VID: &str = "5c8a0d27-0861-4c76-b062-bc938f8c6833";

    fn vid() -> Uuid {
        Uuid::parse_str(VID).unwrap()
    }

    // ── 해피 ──
    #[test]
    fn reads_visitor_cookie() {
        assert_eq!(visitor_id(Some(&format!("_at_vid={VID}"))), Some(vid()));
        assert_eq!(
            visitor_id(Some(&format!("a=1; _at_vid={VID}; b=2"))),
            Some(vid())
        );
        assert_eq!(visitor_id(None), None);
    }

    #[test]
    fn issues_cookie_on_registrable_domain() {
        let c = set_cookie(vid(), "track.client.com");
        assert_eq!(
            c,
            format!(
                "_at_vid={VID}; Domain=client.com; Path=/; Max-Age=31536000; HttpOnly; Secure; SameSite=Lax"
            )
        );
    }

    // ── 경계값·엣지 ──
    #[test]
    fn many_cookies_and_malformed_pairs() {
        let mut parts: Vec<String> = (0..500).map(|i| format!("c{i}=v{i}")).collect();
        parts.push(format!("_at_vid={VID}"));
        assert_eq!(visitor_id(Some(&parts.join("; "))), Some(vid()));

        let messy = format!(";;; =; novalue; =orphan;  _at_vid = {VID} ;x");
        assert_eq!(visitor_id(Some(&messy)), Some(vid()));
        assert_eq!(visitor_id(Some("")), None);
        assert_eq!(visitor_id(Some(";;;")), None);
        assert_eq!(visitor_id(Some("_at_vid=")), None);
        assert_eq!(visitor_id(Some("_at_vid")), None);
    }

    #[test]
    fn name_must_match_exactly() {
        assert_eq!(visitor_id(Some(&format!("_AT_VID={VID}"))), None);
        assert_eq!(visitor_id(Some(&format!("x_at_vid={VID}"))), None);
        assert_eq!(visitor_id(Some(&format!("_at_vid2={VID}"))), None);
    }

    #[test]
    fn duplicate_names_first_valid_wins() {
        let other = "11111111-2222-4333-8444-555555555555";
        assert_eq!(
            visitor_id(Some(&format!("_at_vid={VID}; _at_vid={other}"))),
            Some(vid())
        );
        // 앞의 값이 깨져 있으면 다음 유효한 값을 쓴다.
        assert_eq!(
            visitor_id(Some(&format!("_at_vid=garbage; _at_vid={VID}"))),
            Some(vid())
        );
        assert_eq!(
            visitor_id(Some(
                "_at_vid=00000000-0000-0000-0000-000000000000; _at_vid=x"
            )),
            None,
            "nil UUID는 방문자로 쓰지 않는다"
        );
    }

    #[test]
    fn quoted_value_is_accepted() {
        assert_eq!(visitor_id(Some(&format!("_at_vid=\"{VID}\""))), Some(vid()));
    }

    #[test]
    fn cookie_domain_variants() {
        assert_eq!(
            cookie_domain("track.client.com").as_deref(),
            Some("client.com")
        );
        assert_eq!(cookie_domain("client.com").as_deref(), Some("client.com"));
        assert_eq!(
            cookie_domain("TRACK.Client.COM.").as_deref(),
            Some("client.com")
        );
        assert_eq!(
            cookie_domain("track.shop.client.co.kr").as_deref(),
            Some("client.co.kr")
        );
        assert_eq!(
            cookie_domain("track.client.co.kr").as_deref(),
            Some("client.co.kr")
        );
        assert_eq!(
            cookie_domain("track.client.com:8443").as_deref(),
            Some("client.com")
        );
    }

    #[test]
    fn cookie_domain_skipped_for_ip_localhost_and_hosted() {
        for h in [
            "localhost",
            "localhost:8787",
            "127.0.0.1",
            "127.0.0.1:8787",
            "[::1]",
            "[::1]:8787",
            "::1",
            "[2001:db8::1]",
            "edge.agency.workers.dev",
            "workers.dev",
            "preview.site.pages.dev",
            "intranet",
            "co.kr",
            "",
        ] {
            assert_eq!(cookie_domain(h), None, "{h}");
        }
        let c = set_cookie(vid(), "edge.agency.workers.dev");
        assert!(!c.contains("Domain="), "{c}");
        assert!(c.contains("HttpOnly; Secure; SameSite=Lax"));
    }

    // ── 외부 공격 ──
    #[test]
    fn injection_in_cookie_value_is_ignored() {
        let evil = format!("_at_vid={VID}\r\nSet-Cookie: admin=1");
        // CRLF가 섞인 값은 UUID가 아니므로 버린다.
        assert_eq!(visitor_id(Some(&evil)), None);
        assert_eq!(visitor_id(Some("_at_vid=abc; Domain=evil.com")), None);
    }

    #[test]
    fn host_injection_cannot_reach_set_cookie() {
        for h in [
            "client.com\r\nSet-Cookie: x=1",
            "client.com; Domain=evil.com",
            "evil.com/.client.com",
            "client .com",
        ] {
            assert_eq!(cookie_domain(h), None, "{h:?}");
            let c = set_cookie(vid(), h);
            assert!(!c.contains('\r') && !c.contains('\n'), "{c:?}");
            assert!(!c.contains("evil"), "{c:?}");
        }
    }
}

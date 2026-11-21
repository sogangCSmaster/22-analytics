//! CORS 판단.
//!
//! `/v1/collect`는 쿠키를 주고받으므로(credentials) `*`를 쓸 수 없고, 허용 도메인에 든
//! Origin만 그대로 되돌려준다. `/v1/config`는 공개 정보라 `*`로 답한다.

use crate::http::EdgeResponse;

/// Origin 헤더 최대 길이. 더 길면 정상 브라우저 값이 아니다.
pub const MAX_ORIGIN_LEN: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginCheck {
    /// Origin 헤더 없음 (일부 sendBeacon·서버 호출). core가 Referer로 판단한다.
    Absent,
    /// 허용됨. 응답에 그대로 되돌려줄 값.
    Allowed(String),
    /// `null`, 형식 오류, 허용 목록 밖. 처리하지 않고 CORS 헤더도 붙이지 않는다.
    Denied,
}

/// 브라우저가 보내는 직렬화된 origin(`scheme://host[:port]`)인지 확인하고 정규형을 돌려준다.
/// 경로·쿼리·사용자 정보·제어 문자가 있거나 정규형과 다르면 `None`.
pub fn normalized_origin(origin: &str) -> Option<String> {
    let _ = origin;
    todo!()
}

/// 수집 요청의 Origin을 프로젝트 허용 도메인과 대조한다.
pub fn check_origin(origin: Option<&str>, allowed: &[String]) -> OriginCheck {
    let _ = (origin, allowed);
    todo!()
}

/// 허용된 Origin에 붙이는 응답 헤더.
pub fn credentialed_headers(origin: &str) -> Vec<(String, String)> {
    let _ = origin;
    todo!()
}

/// preflight 응답. 공개 키가 본문에 있어 이 단계에서는 프로젝트를 알 수 없으므로, 형식이 올바른
/// origin이면 허용하고 실제 POST에서 허용 목록을 강제한다 (`null`·형식 오류는 CORS 헤더 없음).
pub fn preflight(origin: Option<&str>) -> EdgeResponse {
    let _ = origin;
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> Vec<String> {
        vec!["client.com".into(), "shop.co.kr".into()]
    }

    #[test]
    fn allowed_origin_is_echoed() {
        assert_eq!(
            check_origin(Some("https://www.client.com"), &allowed()),
            OriginCheck::Allowed("https://www.client.com".into())
        );
        assert_eq!(
            check_origin(Some("http://localhost.client.com:3000"), &allowed()),
            OriginCheck::Allowed("http://localhost.client.com:3000".into())
        );
        assert_eq!(
            check_origin(Some("https://m.shop.co.kr"), &allowed()),
            OriginCheck::Allowed("https://m.shop.co.kr".into())
        );
        let h = credentialed_headers("https://www.client.com");
        let get = |n: &str| h.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
        assert_eq!(
            get("access-control-allow-origin"),
            Some("https://www.client.com")
        );
        assert_eq!(get("access-control-allow-credentials"), Some("true"));
        assert_eq!(get("vary"), Some("Origin"));
    }

    #[test]
    fn absent_origin() {
        assert_eq!(check_origin(None, &allowed()), OriginCheck::Absent);
    }

    // ── 외부 공격 ──
    #[test]
    fn forged_or_odd_origins_are_denied() {
        for o in [
            "https://evil.com",
            "https://evilclient.com",
            "https://client.com.evil.com",
            "null",
            "",
            "client.com",
            "https://www.client.com/path",
            "https://www.client.com?x=1",
            "https://user@www.client.com",
            "https://www.client.com\r\nx-edge-signature: 00",
            "javascript://client.com",
            "file:///etc/passwd",
            "HTTPS://WWW.CLIENT.COM",
        ] {
            assert_eq!(
                check_origin(Some(o), &allowed()),
                OriginCheck::Denied,
                "{o:?}"
            );
        }
        let long = format!("https://{}.client.com", "a".repeat(MAX_ORIGIN_LEN));
        assert_eq!(check_origin(Some(&long), &allowed()), OriginCheck::Denied);
    }

    #[test]
    fn preflight_echoes_well_formed_origin_only() {
        let ok = preflight(Some("https://www.client.com"));
        assert_eq!(ok.status, 204);
        assert_eq!(
            ok.header("access-control-allow-origin"),
            Some("https://www.client.com")
        );
        assert_eq!(ok.header("access-control-allow-credentials"), Some("true"));
        assert!(
            ok.header("access-control-allow-methods")
                .unwrap()
                .contains("POST")
        );
        assert!(ok.header("access-control-max-age").is_some());

        for bad in [None, Some("null"), Some("https://x.com/\r\n")] {
            let r = preflight(bad);
            assert_eq!(r.status, 204);
            assert_eq!(r.header("access-control-allow-origin"), None, "{bad:?}");
        }
    }

    #[test]
    fn normalization() {
        assert_eq!(
            normalized_origin("https://client.com:443").as_deref(),
            None,
            "브라우저는 기본 포트를 생략하므로 정규형과 다르다"
        );
        assert_eq!(
            normalized_origin("https://client.com").as_deref(),
            Some("https://client.com")
        );
    }
}

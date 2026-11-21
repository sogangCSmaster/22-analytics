//! 서버 방문자 쿠키 `_at_vid` 해석. 엣지와 로컬 에뮬레이터가 같이 쓴다.
//! 발급(`Set-Cookie`, Domain 계산)은 배포 환경마다 달라 각자 한다.

use uuid::Uuid;

pub const VISITOR_COOKIE: &str = "_at_vid";

/// `Cookie` 헤더에서 `_at_vid`를 찾는다.
///
/// - 이름은 대소문자를 구분해 정확히 일치해야 한다.
/// - 같은 이름이 여러 번 오면 (경로가 더 구체적인 쿠키가 앞에 온다, RFC 6265 §5.4) 처음으로
///   UUID로 해석되는 값을 쓴다.
/// - 값을 감싼 큰따옴표는 벗긴다.
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

#[cfg(test)]
mod tests {
    use super::*;

    const VID: &str = "5c8a0d27-0861-4c76-b062-bc938f8c6833";

    fn vid() -> Uuid {
        Uuid::parse_str(VID).unwrap()
    }

    #[test]
    fn reads_cookie() {
        assert_eq!(visitor_id(Some(&format!("_at_vid={VID}"))), Some(vid()));
        assert_eq!(
            visitor_id(Some(&format!("a=1; _at_vid=\"{VID}\"; b=2"))),
            Some(vid())
        );
        assert_eq!(visitor_id(None), None);
    }

    #[test]
    fn ignores_invalid_values() {
        for c in [
            "_at_vid=00000000-0000-0000-0000-000000000000",
            "_at_vid=",
            "_at_vid",
            "_AT_VID=5c8a0d27-0861-4c76-b062-bc938f8c6833",
            "_at_vid=not-a-uuid",
        ] {
            assert_eq!(visitor_id(Some(c)), None, "{c}");
        }
        // 첫 값이 잘못돼도 뒤의 올바른 값을 쓴다
        assert_eq!(
            visitor_id(Some(&format!("_at_vid=bad; _at_vid={VID}"))),
            Some(vid())
        );
    }
}

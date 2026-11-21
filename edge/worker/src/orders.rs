//! S2S 주문 API 프록시 (`POST /v1/orders`, `/v1/orders:batch`).
//!
//! 엣지는 비밀 키를 검증하지 않는다 (해시가 원점 Postgres에 있다). 본문을 바이트 그대로 서명해
//! `/internal/orders`로 넘기고, 원점의 상태 코드와 본문을 그대로 돌려준다.
//! 주문은 조용히 Queue에 넣지 않는다: 원점에 닿지 못하면 `503`을 돌려 가맹점 서버가 재시도하게 한다.
//!
//! `Authorization`이 없거나 형식이 잘못된 요청, 단건의 잘못된 멱등 키는 엣지에서 바로 거절한다
//! (원점 부하 절감). 거절 상태·코드는 원점과 같다 ([`analytics_core::orders::api::ApiError`]).

use crate::forward::{self, ForwardRequest};
use crate::http::{EdgeResponse, OrdersKind};
use analytics_core::orders::api::{ApiError, MAX_AUTHORIZATION_LEN, bearer, valid_idempotency_key};

pub use analytics_core::orders::api::{
    HEADER_FORWARDED_AUTHORIZATION, HEADER_FORWARDED_PATH, HEADER_IDEMPOTENCY_KEY,
};
pub use analytics_core::orders::{
    MAX_BATCH_BODY_BYTES as MAX_ORDER_BATCH_BYTES, MAX_IDEMPOTENCY_KEY_LEN,
    MAX_SINGLE_BODY_BYTES as MAX_ORDER_BYTES,
};
/// 원점 장애 시 `Retry-After` (초)
pub const RETRY_AFTER_SECS: u32 = 30;

#[derive(Debug, Clone, Copy)]
pub struct OrdersRequest<'a> {
    pub kind: OrdersKind,
    pub authorization: Option<&'a str>,
    pub idempotency_key: Option<&'a str>,
    pub body: &'a [u8],
}

/// 검증 후 원점 요청을 만든다. 거절이면 클라이언트에 줄 응답.
pub fn build_forward(
    req: &OrdersRequest<'_>,
    origin_url: &str,
    secret: &[u8],
    now_ms: i64,
) -> Result<ForwardRequest, EdgeResponse> {
    if req.body.len() > req.kind.body_limit() {
        return Err(EdgeResponse::api_error(ApiError::TooLarge));
    }
    let authorization = valid_authorization(req.authorization).map_err(EdgeResponse::api_error)?;
    if req.body.is_empty() {
        // 원점은 빈 본문을 JSON 해석 오류로 본다
        return Err(EdgeResponse::api_error(ApiError::BadJson));
    }
    // 전달 헤더도 서명에 포함된다 (원점이 같은 값으로 다시 계산한다).
    // 단건의 잘못된 멱등 키는 원점처럼 422로 거절한다. 일괄은 원점이 헤더 키를 쓰지 않으므로
    // 올바른 키만 전달한다.
    let idempotency_key = match (req.kind, req.idempotency_key) {
        (_, None) => "",
        (_, Some(k)) if valid_idempotency_key(k) => k,
        (OrdersKind::Single, Some(_)) => {
            return Err(EdgeResponse::api_error(ApiError::InvalidIdempotencyKey));
        }
        (OrdersKind::Batch, Some(_)) => "",
    };
    Ok(forward::signed_with(
        origin_url,
        forward::ORDERS_PATH,
        secret,
        req.body.to_vec(),
        now_ms,
        forward::Forwarded {
            path: req.kind.path(),
            authorization,
            idempotency_key,
        },
    ))
}

fn is_header_text(s: &str) -> bool {
    s.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// `Bearer <token>` 형식만 받는다 (scheme은 대소문자 무시). 원점으로 넘길 헤더 원문을 돌려준다.
/// 없거나 형식이 틀리면 `missing_key`, 길이·문자 검사에 실패하면 `invalid_key` (원점과 같은 분류).
fn valid_authorization(v: Option<&str>) -> Result<&str, ApiError> {
    let v = v.map(str::trim).ok_or(ApiError::MissingKey)?;
    if v.len() > MAX_AUTHORIZATION_LEN || !is_header_text(v) {
        return Err(ApiError::InvalidKey);
    }
    bearer(v).map(|_| v).ok_or(ApiError::MissingKey)
}

/// 원점 응답 (wasm 계층이 채운다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginReply {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// 원점 응답을 클라이언트 응답으로. `None`(네트워크 오류·타임아웃)이면 `503` + `Retry-After`.
pub fn map_response(reply: Option<OriginReply>) -> EdgeResponse {
    let Some(reply) = reply else {
        return EdgeResponse::api_error(ApiError::OriginUnavailable)
            .with_header("retry-after", RETRY_AFTER_SECS.to_string());
    };
    // 원점 응답 헤더는 content-type만 옮긴다 (내부 헤더 유출 방지)
    let content_type = reply
        .content_type
        .filter(|c| c.len() <= 256 && is_header_text(c))
        .unwrap_or_else(|| "application/json".into());
    EdgeResponse {
        status: reply.status,
        headers: vec![("content-type".into(), content_type)],
        body: reply.body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forward::tests::{NOW, SECRET, origin_verifies};
    use analytics_core::edge_auth::{HEADER_SIGNATURE, HEADER_TIMESTAMP, SignedRequest, verify};

    const BODY: &[u8] =
        br#"{"event_type":"PURCHASE_CONFIRMED","order_id":"ORD-9901","net_amount":100000}"#;

    fn req(body: &[u8]) -> OrdersRequest<'_> {
        OrdersRequest {
            kind: OrdersKind::Single,
            authorization: Some("Bearer sk_live_abcdefgh12345678"),
            idempotency_key: None,
            body,
        }
    }

    fn verifies(f: &ForwardRequest) -> bool {
        origin_verifies(f, forward::ORDERS_PATH, &f.body, NOW)
    }

    // ── 해피 ──
    #[test]
    fn forwards_raw_body_with_signature_and_auth() {
        let f = build_forward(&req(BODY), "https://origin/", SECRET, NOW).unwrap();
        assert_eq!(f.url, "https://origin/internal/orders");
        assert_eq!(f.body, BODY, "본문은 바이트 그대로");
        assert!(verifies(&f));
        assert_eq!(
            f.header(HEADER_FORWARDED_AUTHORIZATION),
            Some("Bearer sk_live_abcdefgh12345678")
        );
        assert_eq!(f.header(HEADER_FORWARDED_PATH), Some("/v1/orders"));
        assert_eq!(
            f.header("authorization"),
            None,
            "원본 Authorization 헤더는 쓰지 않는다"
        );
    }

    #[test]
    fn batch_path_and_idempotency_key() {
        let mut r = req(BODY);
        r.kind = OrdersKind::Batch;
        r.idempotency_key = Some("ORD-9901:confirm");
        let f = build_forward(&r, "https://origin", SECRET, NOW).unwrap();
        assert_eq!(f.header(HEADER_FORWARDED_PATH), Some("/v1/orders:batch"));
        assert_eq!(f.header(HEADER_IDEMPOTENCY_KEY), Some("ORD-9901:confirm"));
        assert!(verifies(&f), "멱등 키까지 서명된다");
    }

    #[test]
    fn forwarded_headers_are_covered_by_signature() {
        let mut r = req(BODY);
        r.idempotency_key = Some("k-1");
        let f = build_forward(&r, "https://origin", SECRET, NOW).unwrap();
        assert!(verifies(&f));
        let ts: i64 = f.header(HEADER_TIMESTAMP).unwrap().parse().unwrap();
        let sig = f.header(HEADER_SIGNATURE).unwrap();
        let base = SignedRequest {
            internal_path: forward::ORDERS_PATH,
            timestamp_ms: ts,
            body: BODY,
            forwarded_path: "/v1/orders",
            forwarded_authorization: "Bearer sk_live_abcdefgh12345678",
            idempotency_key: "k-1",
        };
        assert!(verify(SECRET, &base, sig, NOW));
        for changed in [
            SignedRequest {
                forwarded_path: "/v1/orders:batch",
                ..base
            },
            SignedRequest {
                forwarded_authorization: "Bearer sk_live_attacker",
                ..base
            },
            SignedRequest {
                idempotency_key: "",
                ..base
            },
            SignedRequest {
                internal_path: forward::COLLECT_PATH,
                ..base
            },
        ] {
            assert!(!verify(SECRET, &changed, sig, NOW), "{changed:?}");
        }
    }

    #[test]
    fn origin_reply_is_passed_verbatim() {
        for (status, body) in [
            (202u16, &br#"{"accepted":true}"#[..]),
            (400, br#"{"error":"invalid_order"}"#),
            (401, br#"{"error":"unauthorized"}"#),
            (500, b"boom"),
        ] {
            let r = map_response(Some(OriginReply {
                status,
                content_type: Some("application/json".into()),
                body: body.to_vec(),
            }));
            assert_eq!(r.status, status);
            assert_eq!(r.body, body);
            assert_eq!(r.header("content-type"), Some("application/json"));
        }
    }

    // ── 멱등성 ──
    #[test]
    fn same_request_same_forward() {
        let a = build_forward(&req(BODY), "https://o", SECRET, NOW).unwrap();
        let b = build_forward(&req(BODY), "https://o", SECRET, NOW).unwrap();
        assert_eq!(a, b);
    }

    // ── 경계값 ──
    #[test]
    fn body_limits_exact() {
        let at = vec![b'x'; MAX_ORDER_BYTES];
        assert!(build_forward(&req(&at), "https://o", SECRET, NOW).is_ok());
        let over = vec![b'x'; MAX_ORDER_BYTES + 1];
        assert_eq!(
            build_forward(&req(&over), "https://o", SECRET, NOW)
                .unwrap_err()
                .status,
            413
        );

        let mut b = req(&over);
        b.kind = OrdersKind::Batch;
        assert!(build_forward(&b, "https://o", SECRET, NOW).is_ok());
        let at = vec![b'x'; MAX_ORDER_BATCH_BYTES];
        b.body = &at;
        assert!(build_forward(&b, "https://o", SECRET, NOW).is_ok());
        let over = vec![b'x'; MAX_ORDER_BATCH_BYTES + 1];
        b.body = &over;
        assert_eq!(
            build_forward(&b, "https://o", SECRET, NOW)
                .unwrap_err()
                .status,
            413
        );
    }

    fn code(r: &EdgeResponse) -> String {
        let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        v["error"].as_str().unwrap().to_owned()
    }

    #[test]
    fn empty_body_rejected_at_edge() {
        let e = build_forward(&req(b""), "https://o", SECRET, NOW).unwrap_err();
        assert_eq!(e.status, 400);
        assert_eq!(code(&e), "bad_json", "원점(ingest)과 같은 코드");
    }

    #[test]
    fn error_codes_match_origin() {
        // 원점 ingest `/v1/orders`와 같은 상태·코드
        let over = vec![b'x'; MAX_ORDER_BYTES + 1];
        let e = build_forward(&req(&over), "https://o", SECRET, NOW).unwrap_err();
        assert_eq!((e.status, code(&e).as_str()), (413, "too_large"));

        let mut r = req(BODY);
        r.authorization = None;
        let e = build_forward(&r, "https://o", SECRET, NOW).unwrap_err();
        assert_eq!((e.status, code(&e).as_str()), (401, "missing_key"));

        r.authorization = Some("Basic dXNlcjpwYXNz");
        let e = build_forward(&r, "https://o", SECRET, NOW).unwrap_err();
        assert_eq!(code(&e), "missing_key");

        let long = format!("Bearer sk_{}", "a".repeat(MAX_AUTHORIZATION_LEN));
        r.authorization = Some(&long);
        let e = build_forward(&r, "https://o", SECRET, NOW).unwrap_err();
        assert_eq!((e.status, code(&e).as_str()), (401, "invalid_key"));

        let down = map_response(None);
        assert_eq!(code(&down), "origin_unavailable");
    }

    // ── 장애 ──
    #[test]
    fn origin_down_is_503_never_queued() {
        let r = map_response(None);
        assert_eq!(r.status, 503);
        assert_eq!(r.header("retry-after"), Some("30"));
    }

    // ── 외부 공격 ──
    #[test]
    fn missing_or_malformed_authorization_is_401() {
        for auth in [
            None,
            Some(""),
            Some("   "),
            Some("Basic dXNlcjpwYXNz"),
            Some("Bearer "),
            Some("Bearer sk_live\r\nx-edge-signature: 00"),
            Some("Bearer sk_live_\u{7f}"),
        ] {
            let mut r = req(BODY);
            r.authorization = auth;
            let e = build_forward(&r, "https://o", SECRET, NOW).unwrap_err();
            assert_eq!(e.status, 401, "{auth:?}");
            assert!(
                !String::from_utf8_lossy(&e.body).contains("sk_live"),
                "입력 반사 금지"
            );
        }
        let long = format!("Bearer sk_{}", "a".repeat(MAX_AUTHORIZATION_LEN));
        let mut r = req(BODY);
        r.authorization = Some(&long);
        assert_eq!(
            build_forward(&r, "https://o", SECRET, NOW)
                .unwrap_err()
                .status,
            401
        );
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        let mut r = req(BODY);
        r.authorization = Some("bearer sk_live_abcdefgh12345678");
        assert!(build_forward(&r, "https://o", SECRET, NOW).is_ok());
    }

    #[test]
    fn bad_idempotency_key_single_is_422_batch_is_dropped() {
        for k in [
            "",
            "a b",
            "a\r\nb",
            &"k".repeat(MAX_IDEMPOTENCY_KEY_LEN + 1),
        ] {
            // 단건: 원점과 같은 422 invalid_idempotency_key
            let mut r = req(BODY);
            r.idempotency_key = Some(k);
            let e = build_forward(&r, "https://o", SECRET, NOW).unwrap_err();
            assert_eq!(
                (e.status, code(&e).as_str()),
                (422, "invalid_idempotency_key"),
                "{k:?}"
            );
            // 일괄: 원점이 헤더 키를 쓰지 않으므로 전달하지 않는다
            r.kind = OrdersKind::Batch;
            let f = build_forward(&r, "https://o", SECRET, NOW).unwrap();
            assert_eq!(f.header(HEADER_IDEMPOTENCY_KEY), None, "{k:?}");
        }
    }

    #[test]
    fn signature_covers_exact_raw_body() {
        // 공백·키 순서까지 원문 그대로 서명되어야 원점이 같은 바이트로 검증할 수 있다.
        let raw = b"{ \"order_id\" : \"A\" ,\n \"net_amount\":1 }";
        let f = build_forward(&req(raw), "https://o", SECRET, NOW).unwrap();
        assert_eq!(f.body, raw);
        assert!(verifies(&f));
        assert!(!origin_verifies(
            &f,
            forward::ORDERS_PATH,
            br#"{"order_id":"A","net_amount":1}"#,
            NOW
        ));
    }

    #[test]
    fn only_whitelisted_headers_reach_origin() {
        let f = build_forward(&req(BODY), "https://o", SECRET, NOW).unwrap();
        let mut names: Vec<_> = f.headers.iter().map(|(k, _)| k.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "content-type",
                HEADER_SIGNATURE,
                HEADER_TIMESTAMP,
                HEADER_FORWARDED_AUTHORIZATION,
                HEADER_FORWARDED_PATH
            ]
        );
    }

    #[test]
    fn origin_reply_headers_are_not_leaked() {
        let r = map_response(Some(OriginReply {
            status: 202,
            content_type: Some("application/json\r\nset-cookie: x=1".into()),
            body: b"{}".to_vec(),
        }));
        assert_eq!(
            r.header("content-type"),
            Some("application/json"),
            "잘못된 content-type은 기본값"
        );
        assert_eq!(r.headers.len(), 1);
    }
}

//! Worker → 원점(ingest) 요청 서명.
//!
//! 서명 = hex(HMAC-SHA256(secret, "{timestamp_ms}.{body}")). 5분 넘게 차이 나면 거부한다.

use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const HEADER_SIGNATURE: &str = "x-edge-signature";
pub const HEADER_TIMESTAMP: &str = "x-edge-timestamp";
const MAX_SKEW_MS: i64 = 5 * 60 * 1000;

type HmacSha256 = Hmac<Sha256>;

pub fn sign(secret: &[u8], timestamp_ms: i64, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC은 모든 키 길이를 받는다");
    mac.update(timestamp_ms.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

pub fn verify(
    secret: &[u8],
    timestamp_ms: i64,
    body: &[u8],
    signature_hex: &str,
    now_ms: i64,
) -> bool {
    if (now_ms - timestamp_ms).abs() > MAX_SKEW_MS {
        return false;
    }
    let Ok(sig) = hex::decode(signature_hex) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC은 모든 키 길이를 받는다");
    mac.update(timestamp_ms.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.verify_slice(&sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_795_219_200_000;

    fn orders_req(body: &[u8]) -> SignedRequest<'_> {
        SignedRequest {
            internal_path: "/internal/orders",
            timestamp_ms: NOW,
            body,
            forwarded_path: "/v1/orders",
            forwarded_authorization: "Bearer sk_live_x",
            idempotency_key: "k1",
        }
    }

    #[test]
    fn roundtrip() {
        let r = SignedRequest::collect(1000, b"{}");
        let sig = sign(b"secret", &r);
        assert!(verify(b"secret", &r, &sig, 1000));
        assert!(!verify(b"other", &r, &sig, 1000));
        assert!(!verify(
            b"secret",
            &SignedRequest::collect(1000, b"{ }"),
            &sig,
            1000
        ));
        assert!(!verify(b"secret", &r, &sig, 1000 + 6 * 60 * 1000));
        assert!(!verify(b"secret", &r, "zz", 1000));
    }

    #[test]
    fn replay_across_internal_paths_fails() {
        // /internal/orders용 서명을 /internal/collect에 재사용할 수 없다 (반대도)
        let body = br#"{"events":[]}"#;
        let o = orders_req(body);
        let sig = sign(b"secret", &o);
        let as_collect = SignedRequest {
            internal_path: "/internal/collect",
            ..o
        };
        assert!(!verify(b"secret", &as_collect, &sig, NOW));
        let c = SignedRequest::collect(NOW, body);
        let csig = sign(b"secret", &c);
        let as_orders = SignedRequest {
            internal_path: "/internal/orders",
            ..c
        };
        assert!(!verify(b"secret", &as_orders, &csig, NOW));
    }

    #[test]
    fn forwarded_headers_are_signed() {
        let body = b"{}";
        let base = orders_req(body);
        let sig = sign(b"secret", &base);
        assert!(verify(b"secret", &base, &sig, NOW));
        for changed in [
            SignedRequest {
                forwarded_path: "/v1/orders:batch",
                ..base
            },
            SignedRequest {
                forwarded_authorization: "Bearer sk_live_other",
                ..base
            },
            SignedRequest {
                forwarded_authorization: "",
                ..base
            },
            SignedRequest {
                idempotency_key: "k2",
                ..base
            },
            SignedRequest {
                idempotency_key: "",
                ..base
            },
            SignedRequest {
                timestamp_ms: NOW + 1,
                ..base
            },
        ] {
            assert!(!verify(b"secret", &changed, &sig, NOW), "{changed:?}");
        }
    }

    #[test]
    fn field_boundaries_cannot_be_shifted() {
        // 줄바꿈으로 필드 경계를 옮기는 값은 검증에서 거부한다
        let body = b"{}";
        let a = SignedRequest {
            forwarded_authorization: "Bearer x\nk1",
            idempotency_key: "",
            ..orders_req(body)
        };
        let b = SignedRequest {
            forwarded_authorization: "Bearer x",
            idempotency_key: "k1",
            ..orders_req(body)
        };
        let sig = sign(b"secret", &b);
        assert!(!verify(b"secret", &a, &sig, NOW));
        assert!(!verify(b"secret", &a, &sign(b"secret", &a), NOW));
    }

    #[test]
    fn legacy_format_signature_rejected() {
        // 이전 형식: HMAC("{ts}.{body}")
        let body = b"{}";
        let mut mac = HmacSha256::new_from_slice(b"secret").unwrap();
        mac.update(format!("{NOW}.").as_bytes());
        mac.update(body);
        let legacy = hex::encode(mac.finalize().into_bytes());
        assert!(!verify(
            b"secret",
            &SignedRequest::collect(NOW, body),
            &legacy,
            NOW
        ));
    }

    #[test]
    fn extreme_timestamps_do_not_panic() {
        for (ts, now) in [
            (i64::MIN, i64::MAX),
            (i64::MAX, i64::MIN),
            (i64::MIN, 0),
            (0, i64::MIN),
            (i64::MAX, 0),
        ] {
            let r = SignedRequest::collect(ts, b"{}");
            let sig = sign(b"secret", &r);
            assert!(!verify(b"secret", &r, &sig, now), "{ts} {now}");
        }
        let r = SignedRequest::collect(i64::MIN, b"{}");
        assert!(verify(b"secret", &r, &sign(b"secret", &r), i64::MIN));
    }
}

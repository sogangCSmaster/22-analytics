//! Worker → 원점(ingest) 요청 서명.
//!
//! 서명 = hex(HMAC-SHA256(secret, canonical)). canonical(v1)은 다음 7줄을 `\n`으로 잇는다:
//!
//! ```text
//! v1
//! {internal_path}              // /internal/collect | /internal/orders
//! {timestamp_ms}
//! {sha256_hex(body)}
//! {x-forwarded-path}           // 없으면 ''
//! {x-forwarded-authorization}  // 없으면 ''
//! {idempotency-key}            // 없으면 ''
//! ```
//!
//! 내부 경로와 전달 헤더까지 서명하므로 한 경로용 서명을 다른 경로에 재사용하거나(교차 재생)
//! 전달 헤더만 바꿔치기할 수 없다. 값에 `\n`이 있으면 필드 경계가 모호해지므로 검증하지 않는다.
//! 타임스탬프가 5분 넘게 차이 나면 거부한다.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

pub const HEADER_SIGNATURE: &str = "x-edge-signature";
pub const HEADER_TIMESTAMP: &str = "x-edge-timestamp";
pub const SIGNATURE_VERSION: &str = "v1";
pub const COLLECT_PATH: &str = "/internal/collect";
pub const ORDERS_PATH: &str = "/internal/orders";
const MAX_SKEW_MS: u64 = 5 * 60 * 1000;

type HmacSha256 = Hmac<Sha256>;

/// 서명 대상. 없는 헤더는 `""`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignedRequest<'a> {
    pub internal_path: &'a str,
    pub timestamp_ms: i64,
    pub body: &'a [u8],
    pub forwarded_path: &'a str,
    pub forwarded_authorization: &'a str,
    pub idempotency_key: &'a str,
}

impl<'a> SignedRequest<'a> {
    /// `/internal/collect` 요청 (전달 헤더 없음).
    pub fn collect(timestamp_ms: i64, body: &'a [u8]) -> Self {
        Self {
            internal_path: COLLECT_PATH,
            timestamp_ms,
            body,
            forwarded_path: "",
            forwarded_authorization: "",
            idempotency_key: "",
        }
    }

    fn well_formed(&self) -> bool {
        [
            self.internal_path,
            self.forwarded_path,
            self.forwarded_authorization,
            self.idempotency_key,
        ]
        .iter()
        .all(|v| !v.contains('\n'))
    }

    fn mac(&self, secret: &[u8]) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC은 모든 키 길이를 받는다");
        let body_hash = hex::encode(Sha256::digest(self.body));
        let canonical = format!(
            "{SIGNATURE_VERSION}\n{}\n{}\n{body_hash}\n{}\n{}\n{}",
            self.internal_path,
            self.timestamp_ms,
            self.forwarded_path,
            self.forwarded_authorization,
            self.idempotency_key,
        );
        mac.update(canonical.as_bytes());
        mac
    }
}

pub fn sign(secret: &[u8], req: &SignedRequest<'_>) -> String {
    hex::encode(req.mac(secret).finalize().into_bytes())
}

pub fn verify(secret: &[u8], req: &SignedRequest<'_>, signature_hex: &str, now_ms: i64) -> bool {
    // abs_diff: i64::MIN 같은 극단값에서도 오버플로하지 않는다
    if now_ms.abs_diff(req.timestamp_ms) > MAX_SKEW_MS || !req.well_formed() {
        return false;
    }
    let Ok(sig) = hex::decode(signature_hex) else {
        return false;
    };
    req.mac(secret).verify_slice(&sig).is_ok()
}

/// 서명에 들어가는 전달 헤더 값 (없으면 `""`). `/internal/collect`는 모두 비어 있다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Forwarded<'a> {
    pub path: &'a str,
    pub authorization: &'a str,
    pub idempotency_key: &'a str,
}

/// 원점에서 받은 헤더 값 그대로 검증한다: 타임스탬프·서명 헤더가 없거나 숫자가 아니면 거부.
pub fn verify_headers(
    secret: &[u8],
    internal_path: &str,
    ts_header: Option<&str>,
    sig_header: Option<&str>,
    body: &[u8],
    forwarded: Forwarded<'_>,
    now_ms: i64,
) -> bool {
    let ts = ts_header.and_then(|v| v.parse::<i64>().ok());
    let (Some(timestamp_ms), Some(sig)) = (ts, sig_header) else {
        return false;
    };
    let req = SignedRequest {
        internal_path,
        timestamp_ms,
        body,
        forwarded_path: forwarded.path,
        forwarded_authorization: forwarded.authorization,
        idempotency_key: forwarded.idempotency_key,
    };
    verify(secret, &req, sig, now_ms)
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
    fn verify_headers_parses_and_checks() {
        let body = br#"{"x":1}"#;
        let o = orders_req(body);
        let sig = sign(b"secret", &o);
        let ts = NOW.to_string();
        let fwd = Forwarded {
            path: o.forwarded_path,
            authorization: o.forwarded_authorization,
            idempotency_key: o.idempotency_key,
        };
        let v = |ts: Option<&str>, sig: Option<&str>, fwd| {
            verify_headers(b"secret", ORDERS_PATH, ts, sig, body, fwd, NOW)
        };
        assert!(v(Some(&ts), Some(&sig), fwd));
        assert!(!v(None, Some(&sig), fwd));
        assert!(!v(Some(&ts), None, fwd));
        assert!(!v(Some("12x"), Some(&sig), fwd));
        assert!(
            !v(Some(&ts), Some(&sig), Forwarded::default()),
            "전달 헤더도 서명 대상"
        );
        // collect: 전달 헤더 없음
        let c = sign(b"secret", &SignedRequest::collect(NOW, body));
        assert!(verify_headers(
            b"secret",
            COLLECT_PATH,
            Some(&ts),
            Some(&c),
            body,
            Forwarded::default(),
            NOW
        ));
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

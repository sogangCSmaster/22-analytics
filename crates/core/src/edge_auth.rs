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

    #[test]
    fn roundtrip() {
        let sig = sign(b"secret", 1000, b"{}");
        assert!(verify(b"secret", 1000, b"{}", &sig, 1000));
        assert!(!verify(b"other", 1000, b"{}", &sig, 1000));
        assert!(!verify(b"secret", 1000, b"{ }", &sig, 1000));
        assert!(!verify(b"secret", 1000, b"{}", &sig, 1000 + 6 * 60 * 1000));
        assert!(!verify(b"secret", 1000, b"{}", "zz", 1000));
    }
}

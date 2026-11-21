use serde::{Deserialize, Serialize};
use std::fmt;

/// ClickHouse `UInt32`, Postgres `projects.id`와 같은 값.
pub type ProjectId = u32;

/// 스니펫에 노출되는 공개 키. 형식: `pk_` + 영숫자 8~48자.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PublicKey(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("잘못된 키 형식")]
pub struct InvalidKey;

impl PublicKey {
    pub fn parse(s: &str) -> Result<Self, InvalidKey> {
        let body = s.strip_prefix("pk_").ok_or(InvalidKey)?;
        let ok_len = (8..=48).contains(&body.len());
        let ok_chars = body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if ok_len && ok_chars {
            Ok(Self(s.to_owned()))
        } else {
            Err(InvalidKey)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PublicKey {
    type Error = InvalidKey;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<PublicKey> for String {
    fn from(k: PublicKey) -> Self {
        k.0
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_public_key() {
        assert!(PublicKey::parse("pk_7Hq2abcdEFGH").is_ok());
        assert!(PublicKey::parse("pk_short123").is_ok());
        assert_eq!(PublicKey::parse("sk_7Hq2abcdEFGH"), Err(InvalidKey));
        assert_eq!(PublicKey::parse("pk_abc"), Err(InvalidKey));
        assert_eq!(PublicKey::parse("pk_abc-defgh"), Err(InvalidKey));
    }
}

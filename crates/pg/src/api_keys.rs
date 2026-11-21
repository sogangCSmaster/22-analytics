//! S2S 비밀 키(`sk_live_...`) 발급·검증.
//!
//! 형식: `sk_live_<prefix 8자>_<난수 40자>` (영숫자). DB에는 조회용 prefix(`sk_live_<8자>`)와
//! 전체 키의 sha256 hex만 저장한다. 키 엔트로피가 충분히 높아(40자 × log2(57) ≈ 233비트)
//! 느린 해시(argon2)가 필요 없고, 요청마다 검증하므로 빠른 해시를 쓴다.
//! 해시 비교는 상수 시간으로 한다.

use sha2::{Digest, Sha256};
use sqlx::{Acquire, PgExecutor, PgPool, Postgres};
use time::OffsetDateTime;

pub const KEY_SCHEME: &str = "sk_live_";
pub const PREFIX_LEN: usize = 8;
pub const SECRET_LEN: usize = 40;
pub const SCOPE_ORDERS_WRITE: &str = "orders:write";

/// `api_keys` 행 (해시는 노출하지 않는다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    pub id: i64,
    pub project_id: i32,
    /// 표시·조회용 `sk_live_XXXXXXXX`
    pub prefix: String,
    pub scopes: Vec<String>,
    pub created_by: Option<i64>,
    pub created_at: OffsetDateTime,
    pub last_used_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

/// 검증된 키.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedKey {
    pub project_id: u32,
    pub key_id: i64,
    pub scopes: Vec<String>,
}

impl VerifiedKey {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// 새 비밀 키 문자열.
pub fn generate() -> String {
    format!(
        "{KEY_SCHEME}{}_{}",
        crate::random_alnum(PREFIX_LEN),
        crate::random_alnum(SECRET_LEN)
    )
}

/// 제시된 키에서 조회용 prefix(`sk_live_XXXXXXXX`)를 꺼낸다. 형식이 틀리면 `None`.
pub fn lookup_prefix(presented: &str) -> Option<&str> {
    let rest = presented.strip_prefix(KEY_SCHEME)?;
    let (p, s) = rest.split_once('_')?;
    let alnum = |x: &str| x.bytes().all(|b| b.is_ascii_alphanumeric());
    (p.len() == PREFIX_LEN && s.len() == SECRET_LEN && alnum(p) && alnum(s))
        .then(|| &presented[..KEY_SCHEME.len() + PREFIX_LEN])
}

/// 전체 키의 sha256 hex.
pub fn hash_key(full: &str) -> String {
    hex::encode(Sha256::digest(full.as_bytes()))
}

/// 상수 시간 비교. 길이가 다르면 즉시 false (길이는 비밀이 아니다).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

type VerifyRow = (i64, i32, String, Vec<String>, Option<OffsetDateTime>);

const SELECT_COLS: &str =
    "id, project_id, prefix, scopes, created_by, created_at, last_used_at, revoked_at";

type KeyRow = (
    i64,
    i32,
    String,
    Vec<String>,
    Option<i64>,
    OffsetDateTime,
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
);

fn to_key(r: KeyRow) -> ApiKey {
    ApiKey {
        id: r.0,
        project_id: r.1,
        prefix: r.2,
        scopes: r.3,
        created_by: r.4,
        created_at: r.5,
        last_used_at: r.6,
        revoked_at: r.7,
    }
}

/// 키를 발급한다. 평문 키는 반환값으로 한 번만 돌려준다.
pub async fn create<'c>(
    conn: impl Acquire<'c, Database = Postgres>,
    project_id: i32,
    created_by: Option<i64>,
) -> anyhow::Result<(String, ApiKey)> {
    let mut tx = conn.begin().await?;
    // prefix가 우연히 겹치면(UNIQUE 위반) 새로 만든다.
    for _ in 0..5 {
        let plain = generate();
        let prefix = lookup_prefix(&plain).expect("생성한 키는 형식이 맞다");
        let res = sqlx::query_as::<_, KeyRow>(sqlx::AssertSqlSafe(format!(
            "INSERT INTO api_keys (project_id, prefix, key_hash, created_by) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (prefix) DO NOTHING RETURNING {SELECT_COLS}"
        )))
        .bind(project_id)
        .bind(prefix)
        .bind(hash_key(&plain))
        .bind(created_by)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = res {
            tx.commit().await?;
            return Ok((plain, to_key(row)));
        }
    }
    anyhow::bail!("키 prefix 충돌이 반복됨")
}

/// 프로젝트의 키 목록 (폐기된 키 포함, 최신순).
pub async fn list(pool: &PgPool, project_id: i32) -> anyhow::Result<Vec<ApiKey>> {
    let rows = sqlx::query_as::<_, KeyRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {SELECT_COLS} FROM api_keys WHERE project_id = $1 ORDER BY id DESC"
    )))
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(to_key).collect())
}

/// 폐기되지 않은 키 수.
pub async fn count_active(pool: &PgPool, project_id: i32) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM api_keys WHERE project_id = $1 AND revoked_at IS NULL",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?)
}

/// 키를 폐기한다. 다른 프로젝트의 키이거나 이미 폐기됐으면 false.
pub async fn revoke<'e>(
    ex: impl PgExecutor<'e>,
    project_id: i32,
    key_id: i64,
) -> anyhow::Result<bool> {
    let n = sqlx::query(
        "UPDATE api_keys SET revoked_at = now() \
         WHERE id = $1 AND project_id = $2 AND revoked_at IS NULL",
    )
    .bind(key_id)
    .bind(project_id)
    .execute(ex)
    .await?
    .rows_affected();
    Ok(n == 1)
}

/// 제시된 키를 검증한다. 없거나, 해시가 다르거나, 폐기됐으면 `None`.
/// 성공하면 `last_used_at`을 갱신한다 (1분에 한 번까지만 쓴다).
pub async fn verify(pool: &PgPool, presented: &str) -> anyhow::Result<Option<VerifiedKey>> {
    let Some(prefix) = lookup_prefix(presented) else {
        return Ok(None);
    };
    let row: Option<VerifyRow> = sqlx::query_as(
        "SELECT id, project_id, key_hash, scopes, revoked_at FROM api_keys WHERE prefix = $1",
    )
    .bind(prefix)
    .fetch_optional(pool)
    .await?;
    // prefix가 없어도 해시 계산·비교는 똑같이 해서 응답 시간 차이를 줄인다.
    let presented_hash = hash_key(presented);
    let (id, project_id, stored, scopes, revoked_at) = match row {
        Some(r) => r,
        None => {
            let _ = ct_eq(presented_hash.as_bytes(), &[b'0'; 64]);
            return Ok(None);
        }
    };
    if !ct_eq(presented_hash.as_bytes(), stored.as_bytes()) || revoked_at.is_some() {
        return Ok(None);
    }
    sqlx::query(
        "UPDATE api_keys SET last_used_at = now() WHERE id = $1 \
         AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(Some(VerifiedKey {
        project_id: project_id as u32,
        key_id: id,
        scopes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_format() {
        let k = generate();
        assert!(k.starts_with(KEY_SCHEME));
        let rest = &k[KEY_SCHEME.len()..];
        let (p, s) = rest.split_once('_').unwrap();
        assert_eq!(p.len(), PREFIX_LEN);
        assert_eq!(s.len(), SECRET_LEN);
        assert!(
            p.bytes()
                .chain(s.bytes())
                .all(|b| b.is_ascii_alphanumeric())
        );
        assert_eq!(lookup_prefix(&k), Some(&k[..KEY_SCHEME.len() + PREFIX_LEN]));
        assert_ne!(generate(), k, "매번 다른 키");
    }

    #[test]
    fn lookup_prefix_rejects_malformed() {
        let good = format!("sk_live_{}_{}", "A".repeat(8), "b".repeat(40));
        assert!(lookup_prefix(&good).is_some());
        for bad in [
            String::new(),
            "sk_live_".into(),
            format!("pk_live_{}_{}", "A".repeat(8), "b".repeat(40)),
            format!("sk_test_{}_{}", "A".repeat(8), "b".repeat(40)),
            format!("sk_live_{}_{}", "A".repeat(7), "b".repeat(41)),
            format!("sk_live_{}_{}", "A".repeat(8), "b".repeat(39)),
            format!("sk_live_{}_{}", "A".repeat(8), "b".repeat(41)),
            format!("sk_live_{}-{}", "A".repeat(8), "b".repeat(40)),
            format!("sk_live_{}_{}'", "A".repeat(8), "b".repeat(39)),
            format!("sk_live_{}_{}", "가".repeat(8), "b".repeat(40)),
        ] {
            assert_eq!(lookup_prefix(&bad), None, "{bad}");
        }
    }

    #[test]
    fn hash_is_sha256_hex() {
        assert_eq!(
            hash_key("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(ct_eq(b"", b""));
    }
}

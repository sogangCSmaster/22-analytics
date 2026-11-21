//! 여러 패키지의 통합 테스트가 공유하는 행 픽스처와 상수.

use serde_json::Value;
use sqlx::PgPool;

/// 테스트용 방문자 ID (`_at_vid` 쿠키 값).
pub const VID: &str = "5c8a0d27-0861-4c76-b062-bc938f8c6833";
/// 테스트용 회원 ID 해시 (`sha256("123456")`, 소문자 hex 64자).
pub const MEMBER_HASH: &str = "8d969eef6ecad3c29a3a629280e686cf0c3f5d5a86aff3ca12020c923adc6c92";
/// 데스크톱 Chrome User-Agent.
pub const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// 조직 1개 + 프로젝트 1개를 넣는다.
///
/// ```ignore
/// let p: u32 = ProjectBuilder::new().status("paused").insert(&db.pg).await;
/// ```
#[derive(Debug, Clone)]
pub struct ProjectBuilder {
    org_name: String,
    name: String,
    status: String,
    settings: Value,
    currency: String,
    salt: String,
    public_key: String,
    origins: Vec<String>,
}

impl Default for ProjectBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectBuilder {
    /// 활성, 설정 `{}`, KRW, 새 salt·공개 키, 허용 origin 없음.
    pub fn new() -> Self {
        Self {
            org_name: "광고주".into(),
            name: "p".into(),
            status: "active".into(),
            settings: Value::Object(Default::default()),
            currency: "KRW".into(),
            salt: analytics_pg::new_salt_hex(),
            public_key: analytics_pg::new_public_key(),
            origins: vec![],
        }
    }

    pub fn org_name(mut self, v: &str) -> Self {
        self.org_name = v.into();
        self
    }

    pub fn name(mut self, v: &str) -> Self {
        self.name = v.into();
        self
    }

    /// `active` | `paused` | `deleted`
    pub fn status(mut self, v: &str) -> Self {
        self.status = v.into();
        self
    }

    /// `projects.settings` JSON (`TrackingSettings`)
    pub fn settings(mut self, v: Value) -> Self {
        self.settings = v;
        self
    }

    pub fn currency(mut self, v: &str) -> Self {
        self.currency = v.into();
        self
    }

    pub fn salt(mut self, v: &str) -> Self {
        self.salt = v.into();
        self
    }

    pub fn public_key(mut self, v: &str) -> Self {
        self.public_key = v.into();
        self
    }

    /// `project_origins`에 넣을 허용 origin (여러 번 호출 가능)
    pub fn origin(mut self, v: &str) -> Self {
        self.origins.push(v.into());
        self
    }

    /// 행을 넣고 프로젝트 ID를 돌려준다. 실패하면 패닉한다 (테스트 전용).
    pub async fn insert(self, pool: &PgPool) -> u32 {
        let org: i64 =
            sqlx::query_scalar("INSERT INTO organizations (name) VALUES ($1) RETURNING id")
                .bind(&self.org_name)
                .fetch_one(pool)
                .await
                .expect("organizations insert");
        let id: i32 = sqlx::query_scalar(
            "INSERT INTO projects (org_id, name, public_key, salt, status, settings, currency) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
        )
        .bind(org)
        .bind(&self.name)
        .bind(&self.public_key)
        .bind(&self.salt)
        .bind(&self.status)
        .bind(&self.settings)
        .bind(&self.currency)
        .fetch_one(pool)
        .await
        .expect("projects insert");
        for o in &self.origins {
            sqlx::query("INSERT INTO project_origins (project_id, origin) VALUES ($1, $2)")
                .bind(id)
                .bind(o)
                .execute(pool)
                .await
                .expect("project_origins insert");
        }
        u32::try_from(id).expect("SERIAL id는 양수")
    }
}

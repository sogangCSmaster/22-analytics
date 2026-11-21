//! 로컬 개발용 시드 데이터.

use sqlx::PgPool;

pub const DEV_PUBLIC_KEY: &str = "pk_devlocal0000000000000000";

pub struct DevSeed {
    pub org_id: i64,
    pub project_id: i32,
    pub public_key: String,
}

/// 개발용 광고주와 프로젝트를 만든다. 이미 있으면 그대로 돌려준다.
pub async fn dev(pool: &PgPool) -> anyhow::Result<DevSeed> {
    let mut tx = pool.begin().await?;
    if let Some((project_id, org_id)) =
        sqlx::query_as::<_, (i32, i64)>("SELECT id, org_id FROM projects WHERE public_key = $1")
            .bind(DEV_PUBLIC_KEY)
            .fetch_optional(&mut *tx)
            .await?
    {
        return Ok(DevSeed {
            org_id,
            project_id,
            public_key: DEV_PUBLIC_KEY.into(),
        });
    }

    let org_id: i64 = sqlx::query_scalar(
        "INSERT INTO organizations (name, memo) VALUES ('개발용 광고주', '로컬 개발 시드') RETURNING id",
    )
    .fetch_one(&mut *tx)
    .await?;

    let settings =
        serde_json::json!({ "ecommerce": true, "outbound_links": true, "scroll_depth": true });
    let project_id: i32 = sqlx::query_scalar(
        "INSERT INTO projects (org_id, name, public_key, salt, settings) VALUES ($1, '로컬 테스트 사이트', $2, $3, $4) RETURNING id",
    )
    .bind(org_id)
    .bind(DEV_PUBLIC_KEY)
    .bind(crate::new_salt_hex())
    .bind(settings)
    .fetch_one(&mut *tx)
    .await?;

    for origin in ["localhost", "127.0.0.1"] {
        sqlx::query("INSERT INTO project_origins (project_id, origin) VALUES ($1, $2)")
            .bind(project_id)
            .bind(origin)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("INSERT INTO attribution_settings (project_id) VALUES ($1)")
        .bind(project_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    tracing::info!(project_id, "개발용 프로젝트 생성");
    Ok(DevSeed {
        org_id,
        project_id,
        public_key: DEV_PUBLIC_KEY.into(),
    })
}

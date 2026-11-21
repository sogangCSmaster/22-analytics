//! `api_keys` 통합 테스트 (격리 DB).

use analytics_pg::api_keys::{self, SCOPE_ORDERS_WRITE};
use analytics_testkit::{ProjectBuilder, TestDb};
use sqlx::PgPool;

async fn project(pool: &PgPool, name: &str) -> i32 {
    let id = ProjectBuilder::new()
        .org_name(name)
        .name(name)
        .insert(pool)
        .await;
    id as i32
}

#[tokio::test(flavor = "multi_thread")]
async fn create_then_verify() {
    let db = TestDb::new().await;
    let p = project(&db.pg, "a").await;
    let (plain, row) = api_keys::create(&db.pg, p, None).await.unwrap();
    assert!(plain.starts_with("sk_live_"));
    assert_eq!(row.project_id, p);
    assert_eq!(row.prefix, &plain[..16]);
    assert_eq!(row.scopes, vec![SCOPE_ORDERS_WRITE.to_owned()]);
    assert!(row.revoked_at.is_none());

    // 평문 키는 DB 어디에도 저장되지 않는다.
    let stored: String = sqlx::query_scalar("SELECT key_hash FROM api_keys WHERE id = $1")
        .bind(row.id)
        .fetch_one(&db.pg)
        .await
        .unwrap();
    assert_ne!(stored, plain);
    assert_eq!(stored, api_keys::hash_key(&plain));
    let leaked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM api_keys WHERE key_hash = $1 OR prefix = $1")
            .bind(&plain)
            .fetch_one(&db.pg)
            .await
            .unwrap();
    assert_eq!(leaked, 0);

    let v = api_keys::verify(&db.pg, &plain).await.unwrap().unwrap();
    assert_eq!(v.project_id, p as u32);
    assert_eq!(v.key_id, row.id);
    assert!(v.has_scope(SCOPE_ORDERS_WRITE));

    let listed = api_keys::list(&db.pg, p).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(
        listed[0].last_used_at.is_some(),
        "검증 성공 시 last_used_at 갱신"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_twice_is_stable() {
    let db = TestDb::new().await;
    let p = project(&db.pg, "a").await;
    let (plain, _) = api_keys::create(&db.pg, p, None).await.unwrap();
    let a = api_keys::verify(&db.pg, &plain).await.unwrap();
    let b = api_keys::verify(&db.pg, &plain).await.unwrap();
    assert_eq!(a, b);
    assert!(a.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_missing_and_tampered_keys_rejected() {
    let db = TestDb::new().await;
    let p = project(&db.pg, "a").await;
    let (plain, _) = api_keys::create(&db.pg, p, None).await.unwrap();

    // 같은 prefix, 다른 비밀 부분 (키 추측)
    let mut tampered = plain.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'a' { 'b' } else { 'a' });
    for bad in [
        String::new(),
        "sk_live_".into(),
        tampered,
        plain.to_uppercase(),
        format!("{plain}x"),
        format!("{}' OR '1'='1", &plain[..16]),
        api_keys::generate(),
    ] {
        assert_eq!(api_keys::verify(&db.pg, &bad).await.unwrap(), None, "{bad}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn revoked_key_rejected_and_revoke_scoped_to_project() {
    let db = TestDb::new().await;
    let a = project(&db.pg, "a").await;
    let b = project(&db.pg, "b").await;
    let (plain, row) = api_keys::create(&db.pg, a, None).await.unwrap();

    // 다른 프로젝트 id로는 폐기할 수 없다 (테넌트 격리)
    assert!(!api_keys::revoke(&db.pg, b, row.id).await.unwrap());
    assert!(api_keys::verify(&db.pg, &plain).await.unwrap().is_some());

    assert!(api_keys::revoke(&db.pg, a, row.id).await.unwrap());
    assert!(
        !api_keys::revoke(&db.pg, a, row.id).await.unwrap(),
        "두 번째 폐기는 false (멱등)"
    );
    assert_eq!(api_keys::verify(&db.pg, &plain).await.unwrap(), None);
    let listed = api_keys::list(&db.pg, a).await.unwrap();
    assert!(listed[0].revoked_at.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn keys_are_bound_to_their_project() {
    let db = TestDb::new().await;
    let a = project(&db.pg, "a").await;
    let b = project(&db.pg, "b").await;
    let (ka, _) = api_keys::create(&db.pg, a, None).await.unwrap();
    let (kb, _) = api_keys::create(&db.pg, b, None).await.unwrap();
    assert_eq!(
        api_keys::verify(&db.pg, &ka)
            .await
            .unwrap()
            .unwrap()
            .project_id,
        a as u32
    );
    assert_eq!(
        api_keys::verify(&db.pg, &kb)
            .await
            .unwrap()
            .unwrap()
            .project_id,
        b as u32
    );
    assert_eq!(api_keys::list(&db.pg, a).await.unwrap().len(), 1);
    assert_eq!(api_keys::list(&db.pg, b).await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn many_keys_unique() {
    let db = TestDb::new().await;
    let p = project(&db.pg, "a").await;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..50 {
        let (k, row) = api_keys::create(&db.pg, p, None).await.unwrap();
        assert!(seen.insert(row.prefix.clone()));
        assert!(api_keys::verify(&db.pg, &k).await.unwrap().is_some());
    }
    assert_eq!(api_keys::list(&db.pg, p).await.unwrap().len(), 50);
}

#[tokio::test(flavor = "multi_thread")]
async fn created_by_must_be_real_user() {
    let db = TestDb::new().await;
    let p = project(&db.pg, "a").await;
    assert!(
        api_keys::create(&db.pg, p, Some(999_999)).await.is_err(),
        "없는 사용자 id는 FK로 거부"
    );
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, name) VALUES ('a@b.c', 'x', 'A') RETURNING id",
    )
    .fetch_one(&db.pg)
    .await
    .unwrap();
    let (_, row) = api_keys::create(&db.pg, p, Some(uid)).await.unwrap();
    assert_eq!(row.created_by, Some(uid));
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_project_rejected() {
    let db = TestDb::new().await;
    assert!(api_keys::create(&db.pg, 424_242, None).await.is_err());
}

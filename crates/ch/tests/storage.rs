//! 0005_storage 마이그레이션: 코덱·컬럼 TTL이 적용되고, 데이터 재작성(mutation) 없이 끝나며,
//! 다시 실행해도 안전하다. 0006_orders_version_index: changed_orders용 version 스킵 인덱스.
//! 서버별 ClickHouse 클라이언트 보호 설정.

use analytics_ch::migrate::{self, MIGRATIONS, statements};
use analytics_testkit::TestDb;

async fn create_table(db: &TestDb, table: &str) -> String {
    db.ch
        .query(&format!("SELECT create_table_query FROM system.tables WHERE database = currentDatabase() AND name = '{table}'"))
        .fetch_one::<String>()
        .await
        .unwrap()
}

async fn codec(db: &TestDb, table: &str, column: &str) -> String {
    db.ch
        .query(
            "SELECT compression_codec FROM system.columns \
             WHERE database = currentDatabase() AND table = ? AND name = ?",
        )
        .bind(table)
        .bind(column)
        .fetch_one::<String>()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn storage_codecs_and_column_ttls() {
    let db = TestDb::new().await;
    assert_eq!(codec(&db, "events", "ts").await, "CODEC(Delta(8), ZSTD(1))");
    assert_eq!(
        codec(&db, "events", "received_at").await,
        "CODEC(Delta(8), ZSTD(1))"
    );
    assert_eq!(codec(&db, "events", "title").await, "CODEC(ZSTD(3))");
    assert_eq!(codec(&db, "events", "referrer").await, "CODEC(ZSTD(3))");

    let events = create_table(&db, "events").await;
    assert!(
        events.contains("`event_id` UUID TTL toDateTime(ts) + toIntervalDay(7)"),
        "{events}"
    );
    // 행 TTL(25개월)은 그대로
    assert!(
        events.contains("TTL toDateTime(ts) + toIntervalMonth(25)"),
        "{events}"
    );
    let oe = create_table(&db, "order_events").await;
    assert!(
        oe.contains("`raw` String TTL toDateTime(received_at) + toIntervalDay(90)"),
        "{oe}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn storage_migration_is_metadata_only_and_idempotent() {
    let db = TestDb::new().await;
    let (_, sql) = MIGRATIONS
        .iter()
        .find(|(v, _)| *v == "0005_storage")
        .expect("0005_storage 등록");
    // 데이터가 있는 상태에서 다시 실행: 오류 없음, mutation(파트 재작성) 없음
    db.ch
        .query(
            "INSERT INTO events (project_id, event_id, ts, received_at, event_name, title) \
             SELECT 1, generateUUIDv4(), now64(3) - number, now64(3), 'page_view', 't' FROM numbers(100)",
        )
        .execute()
        .await
        .unwrap();
    for _ in 0..2 {
        for stmt in statements(sql) {
            db.ch.query(&stmt).execute().await.unwrap();
        }
    }
    let mutations: u64 = db
        .ch
        .query("SELECT count() FROM system.mutations WHERE database = currentDatabase()")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(mutations, 0, "코덱·TTL 변경은 메타데이터만 바꾼다");
    // 러너도 이미 적용된 것으로 본다
    assert!(migrate::run(&db.ch_config).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn orders_version_skip_index_is_metadata_only_and_used() {
    let db = TestDb::new().await;
    let (_, sql) = MIGRATIONS
        .iter()
        .find(|(v, _)| *v == "0006_orders_version_index")
        .expect("0006_orders_version_index 등록");
    let idx: Vec<(String, String)> = db
        .ch
        .query(
            "SELECT name, type FROM system.data_skipping_indices \
             WHERE database = currentDatabase() AND table = 'orders'",
        )
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(idx, vec![("idx_version".to_owned(), "minmax".to_owned())]);
    // 데이터가 있는 상태에서 다시 실행: 오류 없음, MATERIALIZE(파트 재작성) 없음
    db.ch
        .query(
            "INSERT INTO orders (project_id, order_id, status, ordered_at, version) \
             SELECT 1, toString(number), 'confirmed', now64(3), number FROM numbers(100)",
        )
        .execute()
        .await
        .unwrap();
    for _ in 0..2 {
        for stmt in statements(sql) {
            db.ch.query(&stmt).execute().await.unwrap();
        }
    }
    let mutations: u64 = db
        .ch
        .query("SELECT count() FROM system.mutations WHERE database = currentDatabase()")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(
        mutations, 0,
        "인덱스 추가는 새 파트·머지부터 적용 (MATERIALIZE 없음)"
    );
    // changed_orders의 안쪽 조회(version > watermark)가 인덱스를 쓴다
    let plan: Vec<String> = db
        .ch
        .query(
            "EXPLAIN indexes = 1 SELECT order_id FROM orders WHERE project_id = 1 AND version > 50",
        )
        .fetch_all()
        .await
        .unwrap();
    assert!(
        plan.iter().any(|l| l.contains("idx_version")),
        "{}",
        plan.join("\n")
    );
    assert!(migrate::run(&db.ch_config).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn guarded_client_limits_threads_and_memory() {
    // api·jobs 클라이언트: 쿼리 하나가 서버 CPU·메모리를 독점하지 않게 한다
    let db = TestDb::new().await;
    let c = db.ch_config.guarded_client();
    let (threads, memory, exec): (u64, u64, String) = c
        .query(
            "SELECT toUInt64(getSetting('max_threads')), toUInt64(getSetting('max_memory_usage')), \
             toString(getSetting('max_execution_time'))",
        )
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(threads, analytics_ch::GUARD_MAX_THREADS);
    assert_eq!(memory, analytics_ch::GUARD_MAX_MEMORY_USAGE);
    assert_eq!((threads, memory), (2, 1_500_000_000));
    assert_eq!(exec, analytics_ch::MAX_EXECUTION_SECS.to_string());
    // 데이터베이스도 같다
    let name: String = c
        .query("SELECT currentDatabase()")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(name, db.ch_config.database);
}

//! 광고비 CSV 업로드·조회.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

fn upload(project_id: i32, csv: &str) -> Req {
    Req::post(&format!("/v1/projects/{project_id}/ad-spend"))
        .content_type("text/csv")
        .raw(csv.to_owned())
}

fn range(project_id: i32, from: &str, to: &str) -> Req {
    Req::get(&format!(
        "/v1/projects/{project_id}/ad-spend?from={from}&to={to}"
    ))
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_and_query_ad_spend() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let csv = "day,channel,campaign,amount\n\
               2026-11-01,naver_sa,브랜드 키워드,150000\n\
               2026-11-01,meta,,98000\n\
               2026-11-02,meta,\"리타게팅, 11월\",120000\r\n";
    let r = app.call(upload(f.project_id, csv).cookie(&f.analyst)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body, json!({ "rows": 3 }));
    assert_eq!(app.audit_count("ad_spend.upload").await, 1);

    let r = app
        .call(range(f.project_id, "2026-11-01", "2026-11-01").cookie(&f.viewer))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body["from"], "2026-11-01");
    let rows = r.body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "to 포함, 범위 밖 제외");
    assert_eq!(
        rows[0],
        json!({ "day": "2026-11-01", "channel": "meta", "campaign": "", "amount": 98000 })
    );
    let r = app
        .call(range(f.project_id, "2026-10-01", "2026-11-30").cookie(&f.viewer))
        .await;
    let rows = r.body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2]["campaign"], "리타게팅, 11월");
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_is_idempotent_upsert() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let csv = "2026-11-01,meta,,1000\n2026-11-02,meta,,2000\n";
    for _ in 0..2 {
        let r = app.call(upload(f.project_id, csv).cookie(&f.analyst)).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    }
    // 금액 정정 + 같은 키가 파일 안에 두 번 → 마지막 값
    let r = app
        .call(
            upload(
                f.project_id,
                "2026-11-01,meta,,1500\n2026-11-01,META ,,1700\n",
            )
            .cookie(&f.analyst),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body["rows"], 1);
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT channel, amount FROM ad_spend ORDER BY day")
            .fetch_all(&app.db.pg)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![("meta".to_owned(), 1700), ("meta".to_owned(), 2000)],
        "채널 이름은 trim·소문자"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_validation_is_all_or_nothing() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    for bad in [
        "",
        "day,channel,campaign,amount\n",
        "2026-11-01,meta,,-1\n",
        "2026-11-01,meta,,12.5\n",
        "2026-11-01,meta,,1,000\n",
        "2026-11-01,meta,,abc\n",
        "2026-11-01,meta,,99999999999999999999\n",
        "2026-13-01,meta,,100\n",
        "2026-02-30,meta,,100\n",
        "11/01/2026,meta,,100\n",
        "2026-11-01,,,100\n",
        "2026-11-01,meta\n",
        "2026-11-01,meta,,100,extra\n",
        &format!("2026-11-01,{},,100\n", "c".repeat(101)),
        &format!("2026-11-01,meta,{},100\n", "c".repeat(201)),
        "2026-11-01,meta,,100\n2026-11-02,meta,,-5\n",
    ] {
        let r = app.call(upload(f.project_id, bad).cookie(&f.analyst)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad:?}: {}", r.text);
        assert_eq!(r.error_code(), "invalid_input");
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM ad_spend")
        .fetch_one(&app.db.pg)
        .await
        .unwrap();
    assert_eq!(n, 0, "잘못된 행이 하나라도 있으면 아무것도 저장하지 않는다");

    // 0원은 허용
    let r = app
        .call(upload(f.project_id, "2026-11-01,meta,,0\n").cookie(&f.analyst))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_rejects_bad_content_type_and_oversized_body() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let r = app
        .call(
            upload(f.project_id, "2026-11-01,meta,,0\n")
                .content_type("text/plain")
                .cookie(&f.analyst),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "text/plain은 CSRF 위험");

    let line = "2026-11-01,meta,campaign-name-padding-padding,1000\n";
    let big = line.repeat(5 * 1024 * 1024 / line.len() + 10);
    assert!(big.len() > 5 * 1024 * 1024);
    let r = app
        .call(upload(f.project_id, &big).cookie(&f.analyst))
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn ad_spend_range_validation() {
    let app = TestApp::new().await;
    let f = app.fixture().await;
    let p = f.project_id;
    for path in [
        format!("/v1/projects/{p}/ad-spend"),
        format!("/v1/projects/{p}/ad-spend?from=2026-11-01"),
        format!("/v1/projects/{p}/ad-spend?from=2026-11-02&to=2026-11-01"),
        format!("/v1/projects/{p}/ad-spend?from=2026-1-1&to=2026-11-01"),
        format!("/v1/projects/{p}/ad-spend?from=2020-01-01&to=2026-11-01"),
        format!("/v1/projects/{p}/ad-spend?from=x'%20OR%201=1&to=2026-11-01"),
    ] {
        let r = app.call(Req::get(&path).cookie(&f.viewer)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{path}: {}", r.text);
    }
    // 같은 날 from=to, 1년(366일)은 허용
    let r = app
        .call(range(p, "2025-11-01", "2026-11-01").cookie(&f.viewer))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
}

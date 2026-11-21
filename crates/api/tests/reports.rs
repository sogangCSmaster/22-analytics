//! 리포트 엔드포인트 (WP6).
//!
//! 시계는 2026-11-21 12:00 KST로 고정한다 (오늘 = 11-21, 기본 기간 = 11-15..=11-21).

mod common;

use analytics_ch::ProjectScope;
use analytics_ch::attribution::fixtures::{OrderFixture, insert_orders};
use analytics_ch::attribution::{AttributedRow, insert_rows};
use analytics_core::Event;
use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};
use std::sync::Arc;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset, macros::format_description};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// 픽스처
// ---------------------------------------------------------------------------

/// `"2026-11-20 18:45:00"` (Asia/Seoul)
fn kst(s: &str) -> OffsetDateTime {
    let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    PrimitiveDateTime::parse(s, fmt)
        .unwrap()
        .assume_offset(UtcOffset::from_hms(9, 0, 0).unwrap())
}

fn ms(t: OffsetDateTime) -> i64 {
    (t.unix_timestamp_nanos() / 1_000_000) as i64
}

fn now() -> OffsetDateTime {
    kst("2026-11-21 12:00:00")
}

/// 이벤트 하나. `start`면 세션 시작.
#[allow(clippy::too_many_arguments)]
fn ev(
    project: u32,
    name: &str,
    visitor: u128,
    at: &str,
    channel: &str,
    path: &str,
    start: bool,
    props: &[(&str, &str)],
) -> Event {
    let ts = ms(kst(at));
    Event {
        project_id: project,
        event_id: Uuid::new_v4(),
        ts_ms: ts,
        received_at_ms: ts,
        event_name: name.into(),
        visitor_id: Uuid::from_u128(visitor),
        session_id: Uuid::from_u128(visitor + 1000),
        is_session_start: start,
        member_hash: String::new(),
        url: format!("https://shop.test{path}"),
        host: "shop.test".into(),
        path: path.into(),
        title: String::new(),
        referrer: String::new(),
        referrer_host: String::new(),
        utm_source: String::new(),
        utm_medium: String::new(),
        utm_campaign: String::new(),
        utm_content: String::new(),
        utm_term: String::new(),
        click_id: String::new(),
        click_id_type: String::new(),
        channel: channel.into(),
        country: "KR".into(),
        region: String::new(),
        city: String::new(),
        device_type: "desktop".into(),
        browser: String::new(),
        os: String::new(),
        screen_w: 1440,
        props: props
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        ec: None,
    }
}

/// 속성 키로 쓰는 인젝션 문자열
const NASTY_KEY: &str = "'; DROP TABLE events; --";
/// 주문 번호로 쓰는 인젝션 문자열
const NASTY_ORDER: &str = "it's'; DROP TABLE orders; --?{p:String}";
/// 다른 이벤트 이름에 쓰는 인젝션 문자열
const NASTY_EVENT: &str = "page_view' OR '1'='1";

/// 트래픽 픽스처.
///
/// 프로젝트 `p`, 11-20:
/// - v1 (naver_sa): `/` page_view(시작) → `/products/1` page_view → view_item{item_id=A1}
/// - v2 (meta): `/50%_off` page_view(시작) → view_item{item_id=it's} → custom_evt{NASTY_KEY=x}
///
/// 11-21 11:58 (실시간 창 안): v3 direct `/live` page_view(시작)
///
/// 프로젝트 `q` (다른 광고주), 11-20: google_ads `/q-secret` page_view 5건
async fn traffic(app: &TestApp, p: u32, q: u32) {
    let mut evs = vec![
        ev(
            p,
            "page_view",
            1,
            "2026-11-20 10:00:00",
            "naver_sa",
            "/",
            true,
            &[],
        ),
        ev(
            p,
            "page_view",
            1,
            "2026-11-20 10:01:00",
            "naver_sa",
            "/products/1",
            false,
            &[],
        ),
        ev(
            p,
            "view_item",
            1,
            "2026-11-20 10:02:00",
            "naver_sa",
            "/products/1",
            false,
            &[("item_id", "A1")],
        ),
        ev(
            p,
            "page_view",
            2,
            "2026-11-20 11:00:00",
            "meta",
            "/50%_off",
            true,
            &[],
        ),
        ev(
            p,
            "view_item",
            2,
            "2026-11-20 11:01:00",
            "meta",
            "/50%_off",
            false,
            &[("item_id", "it's")],
        ),
        ev(
            p,
            "custom_evt",
            2,
            "2026-11-20 11:02:00",
            "meta",
            "/50%_off",
            false,
            &[(NASTY_KEY, "x")],
        ),
        ev(
            p,
            "page_view",
            3,
            "2026-11-21 11:58:00",
            "direct",
            "/live",
            true,
            &[],
        ),
    ];
    for i in 0..5 {
        evs.push(ev(
            q,
            "page_view",
            90 + i,
            "2026-11-20 10:00:00",
            "google_ads",
            "/q-secret",
            true,
            &[],
        ));
    }
    analytics_ch::rows::insert_events(&app.db.ch, &evs)
        .await
        .unwrap();
}

#[allow(clippy::too_many_arguments)]
fn attr_row(
    p: u32,
    model: &str,
    lookback: u8,
    order_id: &str,
    idx: u16,
    channel: &str,
    touch: Option<&str>,
    ordered_at: &str,
    amount: i64,
    assist: bool,
) -> AttributedRow {
    AttributedRow {
        project_id: p,
        model: model.into(),
        lookback_days: lookback,
        order_id: order_id.into(),
        touch_index: idx,
        channel: channel.into(),
        utm_campaign: String::new(),
        touch_ts: touch.map(|t| ms(kst(t))),
        ordered_at: ms(kst(ordered_at)),
        amount,
        is_assist: assist as u8,
        computed_at: ms(kst("2026-11-21 04:00:00")),
        is_deleted: 0,
    }
}

/// 매출 픽스처 (매뉴얼 1.A): ORD-9901 100,000원 11-20 18:45, 18:10 naver_sa → 18:40 meta.
/// 인젝션 문자열 주문 번호의 주문(기여 연산 전) 5,000원 11-20 09:00.
/// 다른 광고주 프로젝트 `q`에도 같은 주문 번호 ORD-9901 (금액 777,000).
async fn revenue(app: &TestApp, p: u32, q: u32) {
    let at = "2026-11-20 18:45:00";
    let ordered = ms(kst(at));
    insert_orders(
        &app.db.ch,
        &[
            OrderFixture::confirmed(p, "ORD-9901", Some(Uuid::from_u128(1)), 100_000, ordered, 1),
            OrderFixture::confirmed(
                p,
                NASTY_ORDER,
                None,
                5_000,
                ms(kst("2026-11-20 09:00:00")),
                1,
            ),
            OrderFixture::confirmed(q, "ORD-9901", None, 777_000, ordered, 1),
        ],
    )
    .await
    .unwrap();
    let naver = Some("2026-11-20 18:10:00");
    let meta = Some("2026-11-20 18:40:00");
    let mut rows = vec![];
    for (model, a_naver, a_meta) in [
        ("last_click", 0, 100_000),
        ("first_click", 100_000, 0),
        ("linear", 50_000, 50_000),
        ("time_decay", 40_000, 60_000),
        ("raw_sum", 100_000, 100_000),
    ] {
        let assist = model == "last_click";
        rows.push(attr_row(
            p, model, 7, "ORD-9901", 1, "naver_sa", naver, at, a_naver, assist,
        ));
        rows.push(attr_row(
            p, model, 7, "ORD-9901", 2, "meta", meta, at, a_meta, false,
        ));
    }
    insert_rows(&app.db.ch, ProjectScope::authorized(p), &rows)
        .await
        .unwrap();
    let q_rows = vec![attr_row(
        q,
        "last_click",
        7,
        "ORD-9901",
        1,
        "google_ads",
        naver,
        at,
        777_000,
        false,
    )];
    insert_rows(&app.db.ch, ProjectScope::authorized(q), &q_rows)
        .await
        .unwrap();
}

struct Setup {
    app: TestApp,
    f: Fixture,
    /// 다른 광고주의 프로젝트
    other: i32,
}

async fn setup() -> Setup {
    let app = TestApp::at(now()).await;
    let f = app.fixture().await;
    let org_b = app.create_org(&f.root, "광고주 B").await;
    let other = app.create_project(&f.root, org_b, "사이트 B").await;
    Setup { app, f, other }
}

async fn setup_with_data() -> Setup {
    let s = setup().await;
    traffic(&s.app, s.f.project_id as u32, s.other as u32).await;
    revenue(&s.app, s.f.project_id as u32, s.other as u32).await;
    s
}

fn url(p: i32, rest: &str) -> String {
    format!("/v1/projects/{p}/reports/{rest}")
}

/// 퍼센트 인코딩 (경로·쿼리용).
fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 모든 리포트 경로 (프로젝트 id 자리 표시자 포함).
fn all_routes(p: i32) -> Vec<String> {
    let d = "from=2026-11-20&to=2026-11-20";
    vec![
        url(p, &format!("overview?{d}")),
        url(p, &format!("timeseries?{d}")),
        url(p, &format!("channels?{d}")),
        url(p, &format!("pages?{d}")),
        url(p, &format!("events?{d}")),
        url(p, &format!("events/view_item/properties/item_id?{d}")),
        url(p, "realtime"),
        url(p, &format!("funnel?{d}")),
        url(p, &format!("revenue?{d}")),
        url(p, &format!("overlap?{d}")),
        url(p, &format!("orders?{d}")),
        url(p, "orders/ORD-9901"),
    ]
}

fn has_hangul(s: &str) -> bool {
    s.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c))
}

fn cache(r: &Resp) -> &str {
    r.headers
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

async fn get(app: &TestApp, cookie: &str, path: &str) -> Resp {
    app.call(Req::get(path).cookie(cookie)).await
}

async fn ok(app: &TestApp, cookie: &str, path: &str) -> Value {
    let r = get(app, cookie, path).await;
    assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.text);
    r.body
}

// ---------------------------------------------------------------------------
// 해피 케이스
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn traffic_reports_happy_path() {
    let s = setup_with_data().await;
    let (app, p, c) = (&s.app, s.f.project_id, &s.f.viewer);
    let d = "from=2026-11-20&to=2026-11-20";

    let o = ok(app, c, &url(p, &format!("overview?{d}"))).await;
    assert_eq!(
        o["range"],
        json!({ "from": "2026-11-20", "to": "2026-11-20" })
    );
    assert_eq!(
        o["previous_range"],
        json!({ "from": "2026-11-19", "to": "2026-11-19" })
    );
    assert_eq!(o["current"]["pageviews"], 3);
    assert_eq!(o["current"]["visitors"], 2);
    assert_eq!(o["current"]["sessions"], 2);
    assert_eq!(o["current"]["events"], 6);
    assert_eq!(o["current"]["orders"], 2);
    assert_eq!(o["current"]["revenue"], 105_000);
    assert_eq!(o["previous"]["pageviews"], 0);

    let t = ok(app, c, &url(p, "timeseries?from=2026-11-19&to=2026-11-21")).await;
    let pts = t["points"].as_array().unwrap();
    assert_eq!(pts.len(), 3);
    assert_eq!(
        pts[0],
        json!({ "date": "2026-11-19", "visitors": 0, "sessions": 0, "pageviews": 0 })
    );
    assert_eq!(pts[1]["pageviews"], 3);
    assert_eq!(pts[2]["pageviews"], 1);

    let ch = ok(app, c, &url(p, &format!("channels?{d}"))).await;
    let names: Vec<&str> = ch
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["channel"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{ch}");
    assert!(names.contains(&"naver_sa") && names.contains(&"meta"));
    assert!(!names.contains(&"google_ads"), "다른 테넌트 데이터");

    let pg = ok(app, c, &url(p, &format!("pages?{d}"))).await;
    let pg = pg.as_array().unwrap();
    assert_eq!(pg.len(), 3);
    assert!(pg.iter().all(|r| r["path"] != "/q-secret"));
    let pg = ok(app, c, &url(p, &format!("pages?{d}&limit=1"))).await;
    assert_eq!(pg.as_array().unwrap().len(), 1);
    let pg = ok(app, c, &url(p, &format!("pages?{d}&q=products"))).await;
    assert_eq!(
        pg,
        json!([{ "path": "/products/1", "pageviews": 1, "visitors": 1, "entrances": 0 }])
    );

    let e = ok(app, c, &url(p, &format!("events?{d}"))).await;
    let e = e.as_array().unwrap();
    assert_eq!(
        e[0],
        json!({ "event_name": "page_view", "count": 3, "visitors": 2 })
    );
    assert_eq!(
        e[1],
        json!({ "event_name": "view_item", "count": 2, "visitors": 2 })
    );
    assert_eq!(e.len(), 3);

    let pr = ok(
        app,
        c,
        &url(p, &format!("events/view_item/properties/item_id?{d}")),
    )
    .await;
    assert_eq!(
        pr,
        json!([
            { "value": "A1", "count": 1, "visitors": 1 },
            { "value": "it's", "count": 1, "visitors": 1 },
        ])
    );

    let f = ok(app, c, &url(p, &format!("funnel?{d}"))).await;
    assert_eq!(f["steps"][0]["event_name"], "view_item");
    assert_eq!(f["steps"][0]["visitors"], 2);
    assert_eq!(f["steps"].as_array().unwrap().len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn realtime_uses_injected_clock() {
    let s = setup_with_data().await;
    let r = get(&s.app, &s.f.viewer, &url(s.f.project_id, "realtime")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body["now"], "2026-11-21T12:00:00+09:00");
    assert_eq!(r.body["active_5m"], 1);
    assert_eq!(r.body["active_30m"], 1);
    assert_eq!(r.body["per_minute"].as_array().unwrap().len(), 30);
    assert_eq!(
        r.body["top_pages"],
        json!([{ "path": "/live", "pageviews": 1 }])
    );
    assert_eq!(cache(&r), "no-store", "실시간은 항상 캐시 금지");
}

#[tokio::test(flavor = "multi_thread")]
async fn revenue_report_dedups_and_uses_ad_spend() {
    let s = setup_with_data().await;
    let (app, p) = (&s.app, s.f.project_id);
    // 광고비: 기간 안 meta 50,000 + naver_sa 20,000 (두 줄 합산), 기간 밖 meta 999
    let csv = "2026-11-20,meta,a,30000\n2026-11-20,meta,b,20000\n2026-11-20,naver_sa,,20000\n2026-11-25,meta,,999\n";
    let r = app
        .call(
            Req::post(&format!("/v1/projects/{p}/ad-spend"))
                .content_type("text/csv")
                .raw(csv)
                .cookie(&s.f.analyst),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);

    let d = "from=2026-11-20&to=2026-11-20";
    // 기본값: 프로젝트 기본 모델(last_click), 첫 lookback(7)
    let v = ok(app, &s.f.viewer, &url(p, &format!("revenue?{d}"))).await;
    assert_eq!(v["model"], "last_click");
    assert_eq!(v["lookback_days"], 7);
    assert_eq!(v["raw_sum"], 200_000);
    assert_eq!(v["deduplicated"], 100_000);
    assert_eq!(v["duplicate_amount"], 100_000);
    assert_eq!(v["duplicate_rate"], 0.5);
    assert_eq!(v["orders"], 1);
    assert_eq!(v["spend_total"], 70_000);
    let meta = v["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["channel"] == "meta")
        .unwrap()
        .clone();
    assert_eq!(meta["attributed"], 100_000);
    assert_eq!(meta["spend"], 50_000);
    assert_eq!(meta["roas"], 2.0);
    assert!(
        v["channels"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["channel"] != "google_ads"),
        "다른 테넌트 기여 결과"
    );

    let v = ok(
        app,
        &s.f.viewer,
        &url(p, &format!("revenue?{d}&model=linear&lookback=7")),
    )
    .await;
    assert_eq!(v["model"], "linear");
    assert_eq!(v["deduplicated"], 100_000);
    for c in v["channels"].as_array().unwrap() {
        assert_eq!(c["attributed"], 50_000, "{c}");
    }
    // 설정된 lookback 30일: 연산 결과가 없으므로 0
    let v = ok(
        app,
        &s.f.viewer,
        &url(p, &format!("revenue?{d}&lookback=30")),
    )
    .await;
    assert_eq!(v["lookback_days"], 30);
    assert_eq!(v["deduplicated"], 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn revenue_defaults_follow_attribution_settings() {
    let s = setup_with_data().await;
    let (app, p) = (&s.app, s.f.project_id);
    let r = app
        .call(
            Req::put(&format!("/v1/projects/{p}/attribution-settings"))
                .cookie(&s.f.analyst)
                .json(json!({
                    "lookback_days": [14, 3], "default_model": "linear", "half_life_hours": 168,
                    "include_organic": true, "include_referral": true
                })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let v = ok(
        app,
        &s.f.viewer,
        &url(p, "revenue?from=2026-11-20&to=2026-11-20"),
    )
    .await;
    assert_eq!(v["model"], "linear");
    assert_eq!(v["lookback_days"], 3, "가장 짧은(첫) lookback");
    // 더 이상 설정에 없는 7일은 거부
    let r = get(
        app,
        &s.f.viewer,
        &url(p, "revenue?from=2026-11-20&to=2026-11-20&lookback=7"),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text);
    let r = get(app, &s.f.viewer, &url(p, "overlap?lookback=7")).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = get(app, &s.f.viewer, &url(p, "orders/ORD-9901?lookback=7")).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn overlap_orders_and_order_detail() {
    let s = setup_with_data().await;
    let (app, p, c) = (&s.app, s.f.project_id, &s.f.viewer);
    let d = "from=2026-11-20&to=2026-11-20";

    let o = ok(app, c, &url(p, &format!("overlap?{d}"))).await;
    assert_eq!(o["lookback_days"], 7);
    assert_eq!(o["multi_channel_orders"], 1);
    assert_eq!(
        o["paths"],
        json!([{ "channels": ["naver_sa", "meta"], "orders": 1 }])
    );

    let l = ok(app, c, &url(p, &format!("orders?{d}"))).await;
    assert_eq!(l["total"], 2);
    let rows = l["rows"].as_array().unwrap();
    assert_eq!(rows[0]["order_id"], "ORD-9901", "최신순");
    assert_eq!(rows[0]["last_click_channel"], "meta");
    assert_eq!(rows[1]["order_id"], NASTY_ORDER);
    assert!(rows[1]["last_click_channel"].is_null());
    // 페이지네이션
    let l = ok(app, c, &url(p, &format!("orders?{d}&limit=1&offset=1"))).await;
    assert_eq!(l["total"], 2);
    assert_eq!(l["rows"].as_array().unwrap().len(), 1);
    assert_eq!(l["rows"][0]["order_id"], NASTY_ORDER);
    let l = ok(app, c, &url(p, &format!("orders?{d}&offset=10"))).await;
    assert_eq!(l["rows"], json!([]));

    let od = ok(app, c, &url(p, "orders/ORD-9901")).await;
    assert_eq!(
        od["order"]["effective_amount"], 100_000,
        "다른 테넌트의 같은 주문 번호와 섞이지 않음"
    );
    assert_eq!(od["lookback_days"], 7);
    assert_eq!(od["touches"].as_array().unwrap().len(), 2);
    assert_eq!(od["shares"].as_array().unwrap().len(), 5);

    let r = get(app, c, &url(p, "orders/ORD-NOPE")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.error_code(), "not_found");
}

// ---------------------------------------------------------------------------
// 기본 기간·시계·캐시
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn default_range_is_last_7_days_in_seoul() {
    let s = setup().await;
    let v = ok(&s.app, &s.f.viewer, &url(s.f.project_id, "timeseries")).await;
    assert_eq!(
        v["range"],
        json!({ "from": "2026-11-15", "to": "2026-11-21" })
    );
    assert_eq!(v["points"].as_array().unwrap().len(), 7);
}

#[tokio::test(flavor = "multi_thread")]
async fn default_range_uses_seoul_date_across_utc_midnight() {
    // UTC 11-20 15:30 = KST 11-21 00:30 → 오늘은 11-21
    let now = OffsetDateTime::parse(
        "2026-11-20T15:30:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let app = TestApp::at(now).await;
    let root = app.super_admin().await;
    let org = app.create_org(&root, "광고주").await;
    let p = app.create_project(&root, org, "사이트").await;
    let v = ok(&app, &root, &url(p, "overview")).await;
    assert_eq!(
        v["range"],
        json!({ "from": "2026-11-15", "to": "2026-11-21" })
    );
    let r = get(
        &app,
        &root,
        &url(p, "overview?from=2026-11-20&to=2026-11-20"),
    )
    .await;
    assert_eq!(cache(&r), "private, max-age=60", "KST 기준 어제는 닫힌 날");
}

#[tokio::test(flavor = "multi_thread")]
async fn cache_headers_depend_on_closed_range() {
    let s = setup().await;
    let (app, p, c) = (&s.app, s.f.project_id, &s.f.viewer);
    for (q, want) in [
        ("from=2026-11-14&to=2026-11-20", "private, max-age=60"),
        ("from=2026-11-14&to=2026-11-21", "no-store"),
        ("from=2026-11-22&to=2026-11-30", "no-store"),
        ("", "no-store"),
    ] {
        for route in [
            "overview",
            "timeseries",
            "channels",
            "pages",
            "events",
            "funnel",
            "revenue",
            "overlap",
            "orders",
        ] {
            let r = get(app, c, &url(p, &format!("{route}?{q}"))).await;
            assert_eq!(r.status, StatusCode::OK, "{route}?{q}: {}", r.text);
            assert_eq!(cache(&r), want, "{route}?{q}");
        }
        let r = get(
            app,
            c,
            &url(p, &format!("events/view_item/properties/item_id?{q}")),
        )
        .await;
        assert_eq!(cache(&r), want, "properties?{q}");
    }
    // 오류 응답과 다른 라우트는 전역 기본값(no-store)
    let r = get(app, c, &url(p, "overview?from=bad&to=2026-11-20")).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(cache(&r), "no-store");
    let r = get(app, c, &format!("/v1/projects/{p}")).await;
    assert_eq!(cache(&r), "no-store");
    let r = get(app, c, &url(p, "orders/ORD-NOPE")).await;
    assert_eq!(cache(&r), "no-store");
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_requests_are_identical() {
    let s = setup_with_data().await;
    for path in all_routes(s.f.project_id) {
        let a = ok(&s.app, &s.f.viewer, &path).await;
        let b = ok(&s.app, &s.f.viewer, &path).await;
        assert_eq!(a, b, "{path}");
    }
}

// ---------------------------------------------------------------------------
// 검증
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn invalid_inputs_are_400_with_korean_message() {
    let s = setup().await;
    let (app, p, c) = (&s.app, s.f.project_id, &s.f.viewer);
    let long_q = "a".repeat(201);
    let long_key = "k".repeat(65);
    let long_order = "o".repeat(129);
    let cases = [
        url(p, "overview?from=2026-13-01&to=2026-11-20"),
        url(p, "overview?from=2026-11-20&to=2026-11-31"),
        url(p, "overview?from=20261120&to=2026-11-20"),
        url(p, "overview?from=2026-11-21&to=2026-11-20"),
        url(p, "overview?from=2025-01-01&to=2026-11-20"),
        url(p, "overview?from=2026-11-20"),
        url(p, "overview?to=2026-11-20"),
        url(p, "timeseries?from=&to="),
        url(p, "pages?limit=0"),
        url(p, "pages?limit=1001"),
        url(p, "pages?limit=-1"),
        url(p, "pages?limit=abc"),
        url(p, &format!("pages?q={long_q}")),
        url(p, "events?limit=0"),
        url(p, "events?limit=99999999999"),
        url(p, "events/view_item/properties/item_id?limit=1001"),
        url(p, &format!("events/view_item/properties/{long_key}")),
        url(p, &format!("events/{}/properties/item_id", "e".repeat(65))),
        url(p, "revenue?model=raw_sum"),
        url(p, "revenue?model=bogus"),
        url(p, "revenue?model="),
        url(p, "revenue?lookback=14"),
        url(p, "revenue?lookback=0"),
        url(p, "revenue?lookback=256"),
        url(p, "overlap?limit=0"),
        url(p, "overlap?lookback=1"),
        url(p, "orders?limit=1001"),
        url(p, "orders?offset=-1"),
        url(p, "orders?offset=100001"),
        url(p, "orders/ORD-1?lookback=99"),
        url(p, &format!("orders/{long_order}")),
    ];
    for path in cases {
        let r = get(app, c, &path).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{path}: {}", r.text);
        assert_eq!(r.error_code(), "invalid_input", "{path}");
        let msg = r.body["message"].as_str().unwrap_or("");
        assert!(has_hangul(msg), "{path}: 한국어 메시지 {msg}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn boundary_values_are_accepted() {
    let s = setup().await;
    let (app, p, c) = (&s.app, s.f.project_id, &s.f.viewer);
    for path in [
        // 400일 (양 끝 포함)은 허용
        url(p, "overview?from=2025-10-18&to=2026-11-21"),
        url(p, "timeseries?from=2026-11-21&to=2026-11-21"),
        url(p, "pages?limit=1"),
        url(p, "pages?limit=1000"),
        url(p, "pages?q="),
        url(p, &format!("pages?q={}", "a".repeat(200))),
        url(p, "events?limit=1000"),
        url(
            p,
            &format!("events/view_item/properties/{}", "k".repeat(64)),
        ),
        url(p, "overlap?limit=1000&lookback=30"),
        url(p, "orders?limit=1000&offset=100000"),
        url(p, "revenue?model=time_decay&lookback=30"),
        url(p, "revenue?model=first_click"),
    ] {
        let r = get(app, c, &path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.text);
    }
    let r = get(app, c, &url(p, "overview?from=2025-10-17&to=2026-11-21")).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "401일");
    // 빈 프로젝트: 빈 목록과 0
    let v = ok(app, c, &url(p, "overview")).await;
    assert_eq!(v["current"]["pageviews"], 0);
    assert_eq!(ok(app, c, &url(p, "channels")).await, json!([]));
    let v = ok(app, c, &url(p, "revenue")).await;
    assert_eq!(v["deduplicated"], 0);
    assert!(v["roas"].is_null());
}

// ---------------------------------------------------------------------------
// 외부 공격
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn other_tenant_projects_are_404_on_every_route() {
    let s = setup_with_data().await;
    for cookie in [&s.f.admin, &s.f.analyst, &s.f.viewer] {
        for path in all_routes(s.other).into_iter().chain(all_routes(999_999)) {
            let r = get(&s.app, cookie, &path).await;
            assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}: {}", r.text);
            assert_eq!(r.error_code(), "not_found");
            assert!(!r.text.contains("q-secret") && !r.text.contains("777000"));
        }
    }
    // 숫자가 아닌 프로젝트 id
    for path in all_routes(0)
        .iter()
        .map(|p| p.replacen("/0/", "/1 OR 1=1/", 1))
    {
        let r = get(&s.app, &s.f.viewer, &path.replace(' ', "%20")).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn super_admin_and_every_member_role_can_read() {
    let s = setup_with_data().await;
    for cookie in [&s.f.root, &s.f.admin, &s.f.analyst, &s.f.viewer] {
        for path in all_routes(s.f.project_id) {
            let r = get(&s.app, cookie, &path).await;
            assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.text);
        }
    }
    // 슈퍼 관리자는 다른 광고주 프로젝트도 읽는다
    let v = ok(&s.app, &s.f.root, &url(s.other, "orders/ORD-9901")).await;
    assert_eq!(v["order"]["effective_amount"], 777_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthenticated_is_401_on_every_route() {
    let s = setup_with_data().await;
    for path in all_routes(s.f.project_id) {
        let r = s.app.call(Req::get(&path)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}");
        let r = s
            .app
            .call(Req::get(&path).cookie("at_session=forged-token-value"))
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn injection_strings_are_bound_as_literals() {
    let s = setup_with_data().await;
    let (app, p, c) = (&s.app, s.f.project_id, &s.f.viewer);
    let d = "from=2026-11-20&to=2026-11-20";

    // 경로 필터: `%`·`_`는 와일드카드가 아님
    let v = ok(app, c, &url(p, &format!("pages?{d}&q={}", enc("50%_off")))).await;
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["path"], "/50%_off");
    let v = ok(app, c, &url(p, &format!("pages?{d}&q={}", enc("%")))).await;
    assert_eq!(v.as_array().unwrap().len(), 1);
    for nasty in [
        "' OR '1'='1",
        "'; DROP TABLE daily_pages; --",
        "?",
        "{p:String}",
        "\\",
    ] {
        let v = ok(app, c, &url(p, &format!("pages?{d}&q={}", enc(nasty)))).await;
        assert_eq!(v, json!([]), "{nasty}");
    }

    // 속성 키 인젝션 문자열 → 실제 그 키의 값
    let v = ok(
        app,
        c,
        &url(
            p,
            &format!("events/custom_evt/properties/{}?{d}", enc(NASTY_KEY)),
        ),
    )
    .await;
    assert_eq!(v, json!([{ "value": "x", "count": 1, "visitors": 1 }]));
    // 이벤트 이름 인젝션 → 빈 결과
    let v = ok(
        app,
        c,
        &url(
            p,
            &format!("events/{}/properties/item_id?{d}", enc(NASTY_EVENT)),
        ),
    )
    .await;
    assert_eq!(v, json!([]));

    // 주문 번호 인젝션 문자열 → 그 주문
    let v = ok(app, c, &url(p, &format!("orders/{}", enc(NASTY_ORDER)))).await;
    assert_eq!(v["order"]["order_id"], NASTY_ORDER);
    assert_eq!(v["order"]["effective_amount"], 5_000);
    assert_eq!(v["shares"], json!([]));
    let r = get(
        app,
        c,
        &url(p, &format!("orders/{}", enc("ORD-9901' OR '1'='1"))),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // 테이블이 그대로 있다
    let v = ok(app, c, &url(p, &format!("overview?{d}"))).await;
    assert_eq!(v["current"]["pageviews"], 3);
    assert_eq!(v["current"]["orders"], 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn unicode_values_round_trip() {
    let s = setup().await;
    let p = s.f.project_id as u32;
    let evs = vec![
        ev(
            p,
            "page_view",
            1,
            "2026-11-20 10:00:00",
            "direct",
            "/상품/신발",
            true,
            &[],
        ),
        ev(
            p,
            "view_item",
            1,
            "2026-11-20 10:01:00",
            "direct",
            "/상품/신발",
            false,
            &[("카테고리", "운동화 👟")],
        ),
    ];
    analytics_ch::rows::insert_events(&s.app.db.ch, &evs)
        .await
        .unwrap();
    let d = "from=2026-11-20&to=2026-11-20";
    let v = ok(
        &s.app,
        &s.f.viewer,
        &url(s.f.project_id, &format!("pages?{d}&q={}", enc("신발"))),
    )
    .await;
    assert_eq!(v[0]["path"], "/상품/신발");
    let v = ok(
        &s.app,
        &s.f.viewer,
        &url(
            s.f.project_id,
            &format!("events/view_item/properties/{}?{d}", enc("카테고리")),
        ),
    )
    .await;
    assert_eq!(v[0]["value"], "운동화 👟");
}

// ---------------------------------------------------------------------------
// 장애
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn clickhouse_down_is_503_on_every_route() {
    let app = TestApp::build(
        |_| {},
        |mut s| {
            let mut cfg = analytics_ch::ChConfig::from_env();
            cfg.url = "http://127.0.0.1:1".into();
            s.ch = cfg.client();
            s.with_clock(Arc::new(analytics_api::clock::FixedClock(now())))
        },
    )
    .await;
    let f = app.fixture().await;
    for path in all_routes(f.project_id) {
        let r = get(&app, &f.viewer, &path).await;
        assert_eq!(
            r.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{path}: {}",
            r.text
        );
        assert_eq!(r.error_code(), "storage_unavailable");
        assert!(!r.text.contains("127.0.0.1"), "내부 정보 노출");
        assert_eq!(cache(&r), "no-store");
    }
    // Postgres만 쓰는 라우트는 영향 없음
    let r = get(&app, &f.viewer, &format!("/v1/projects/{}", f.project_id)).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_table_is_503_and_other_reports_still_work() {
    let s = setup_with_data().await;
    s.app
        .db
        .ch
        .query("DROP TABLE daily_channel")
        .execute()
        .await
        .unwrap();
    let r = get(&s.app, &s.f.viewer, &url(s.f.project_id, "channels")).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{}", r.text);
    assert_eq!(r.error_code(), "storage_unavailable");
    let r = get(&s.app, &s.f.viewer, &url(s.f.project_id, "pages")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
}

// ---------------------------------------------------------------------------
// 광고비 합계 넘침·채널 이름 대소문자
// ---------------------------------------------------------------------------

async fn insert_spend(app: &TestApp, p: i32, channel: &str, campaign: &str, amount: i64) {
    sqlx::query(
        "INSERT INTO ad_spend (project_id, day, channel, campaign, amount) \
         VALUES ($1, '2026-11-20', $2, $3, $4)",
    )
    .bind(p)
    .bind(channel)
    .bind(campaign)
    .bind(amount)
    .execute(&app.db.pg)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn huge_ad_spend_saturates_instead_of_failing() {
    let s = setup_with_data().await;
    let (app, p) = (&s.app, s.f.project_id);
    // 업로드 상한 이전에 들어간 값처럼, 채널 합계와 전체 합계가 i64를 넘는 데이터
    for (channel, campaign) in [
        ("meta", "a"),
        ("meta", "b"),
        ("naver_sa", "a"),
        ("kakao", "a"),
    ] {
        insert_spend(app, p, channel, campaign, i64::MAX - 1).await;
    }
    let v = ok(
        app,
        &s.f.viewer,
        &url(p, "revenue?from=2026-11-20&to=2026-11-20"),
    )
    .await;
    assert_eq!(v["spend_total"], i64::MAX, "{v}");
    for c in v["channels"].as_array().unwrap() {
        assert!(c["spend"].as_i64().unwrap() > 0, "{c}");
    }
    let meta = v["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["channel"] == "meta")
        .unwrap()
        .clone();
    assert_eq!(meta["spend"], i64::MAX);
}

#[tokio::test(flavor = "multi_thread")]
async fn spend_joins_channels_case_insensitively() {
    let s = setup_with_data().await;
    let (app, p) = (&s.app, s.f.project_id);
    // 대소문자만 다른 옛 행(규칙 이름을 그대로 쓰던 시절)도 같은 채널로 합친다.
    insert_spend(app, p, "Meta", "a", 1_000).await;
    insert_spend(app, p, "meta", "b", 49_000).await;
    insert_spend(app, p, "KAKAO", "", 5_000).await;
    let v = ok(
        app,
        &s.f.viewer,
        &url(p, "revenue?from=2026-11-20&to=2026-11-20"),
    )
    .await;
    let channels = v["channels"].as_array().unwrap();
    let names: Vec<&str> = channels
        .iter()
        .map(|c| c["channel"].as_str().unwrap())
        .collect();
    assert_eq!(
        names
            .iter()
            .filter(|n| n.eq_ignore_ascii_case("meta"))
            .count(),
        1,
        "{names:?}"
    );
    let meta = channels.iter().find(|c| c["channel"] == "meta").unwrap();
    assert_eq!(meta["spend"], 50_000);
    assert_eq!(meta["roas"], 2.0);
    let kakao = channels
        .iter()
        .find(|c| c["channel"].as_str().unwrap().eq_ignore_ascii_case("kakao"))
        .unwrap();
    assert_eq!(kakao["spend"], 5_000);
    assert_eq!(v["spend_total"], 55_000);
}

// ---------------------------------------------------------------------------
// 기여 설정 읽기
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn reports_ignore_model_where_it_does_not_apply() {
    let s = setup_with_data().await;
    let (app, p) = (&s.app, s.f.project_id);
    // 주문·중복 경로는 모델과 무관하다: 잘못된 model 값 때문에 실패하지 않는다.
    for path in [
        "overlap?from=2026-11-20&to=2026-11-20&model=bogus",
        "orders?from=2026-11-20&to=2026-11-20&model=raw_sum",
        "orders/ORD-9901?model=nope",
    ] {
        ok(app, &s.f.viewer, &url(p, path)).await;
    }
    let r = get(app, &s.f.viewer, &url(p, "revenue?model=bogus")).await;
    assert_eq!(
        r.status,
        StatusCode::BAD_REQUEST,
        "매출 리포트는 여전히 검사"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reading_reports_does_not_write_attribution_settings() {
    let s = setup_with_data().await;
    let (app, p) = (&s.app, s.f.project_id);
    sqlx::query("DELETE FROM attribution_settings WHERE project_id = $1")
        .bind(p)
        .execute(&app.db.pg)
        .await
        .unwrap();
    let v = ok(
        app,
        &s.f.viewer,
        &url(p, "revenue?from=2026-11-20&to=2026-11-20"),
    )
    .await;
    assert_eq!(v["model"], "last_click");
    assert_eq!(v["lookback_days"], 7);
    ok(
        app,
        &s.f.viewer,
        &url(p, "orders?from=2026-11-20&to=2026-11-20"),
    )
    .await;
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM attribution_settings WHERE project_id = $1")
            .bind(p)
            .fetch_one(&app.db.pg)
            .await
            .unwrap();
    assert_eq!(n, 0, "읽기 경로(viewer)가 설정 행을 만들지 않는다");
}

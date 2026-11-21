//! 데모 서버 테스트 (네이티브). 분류: 해피·멱등·경계·희귀·외부 입력.

use super::*;
use crate::logic::dates::{DateRange, Day};
use crate::logic::query::{self, Report};
use analytics_core::attribution::Model;
use serde::de::DeserializeOwned;
use serde_json::json;

const HOUR: i64 = 3_600_000;
const SEOUL: i64 = 9 * HOUR;

/// 2026-11-21(토) 서울 `h`시 `m`분의 epoch ms
fn at(day: Day, h: i64, m: i64) -> i64 {
    day.0 as i64 * 86_400_000 - SEOUL + h * HOUR + m * 60_000
}

fn today() -> Day {
    Day::from_ymd(2026, 11, 21).unwrap()
}

fn now() -> i64 {
    at(today(), 15, 30)
}

fn range(days: i32) -> DateRange {
    DateRange {
        from: today().add(-(days - 1)),
        to: today(),
    }
}

fn login(s: &mut DemoServer) {
    let (st, body) = s.handle(
        Method::Post,
        "/v1/auth/login",
        Some(r#"{"email":"x@y.z","password":"pw"}"#),
    );
    assert_eq!(st, 200, "{body}");
}

fn srv() -> DemoServer {
    let mut s = DemoServer::new(now());
    login(&mut s);
    s
}

fn get<T: DeserializeOwned>(s: &mut DemoServer, path: &str) -> T {
    let (st, body) = s.handle(Method::Get, path, None);
    assert!((200..300).contains(&st), "{path} → {st} {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("{path}: {e}\n{body}"))
}

fn send<T: DeserializeOwned>(s: &mut DemoServer, m: Method, path: &str, body: &str) -> T {
    let (st, text) = s.handle(m, path, Some(body));
    assert!((200..300).contains(&st), "{path} → {st} {text}");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}\n{text}"))
}

/// 오류 응답: (상태, error 코드)
fn err(s: &mut DemoServer, m: Method, path: &str, body: Option<&str>) -> (u16, String) {
    let (st, text) = s.handle(m, path, body);
    let b: ApiErrorBody =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}\n{text}"));
    assert!(!b.message.is_empty());
    (st, b.error)
}

fn overview(s: &mut DemoServer, pid: i32, r: DateRange) -> OverviewReport {
    get(s, &query::report(pid, Report::Overview, r))
}

fn revenue(s: &mut DemoServer, pid: i32, r: DateRange, m: Model, lb: u8) -> RevenueReport {
    get(s, &query::revenue(pid, r, m, lb))
}

const MODELS: [Model; 4] = [
    Model::LastClick,
    Model::FirstClick,
    Model::Linear,
    Model::TimeDecay,
];

// ---------------------------------------------------------------- 인증

#[test]
fn login_accepts_any_credentials() {
    let long = "가".repeat(100_000);
    let cases = [
        json!({"email": DEMO_EMAIL, "password": "demo"}),
        json!({"email": "", "password": ""}),
        json!({"email": "<script>alert(1)</script>", "password": "' OR 1=1 --"}),
        json!({"email": long, "password": long}),
        json!({"email": "\u{0}\n\r", "password": "🔒"}),
    ];
    for c in cases {
        let mut s = DemoServer::new(now());
        let me: MeResponse = send(&mut s, Method::Post, "/v1/auth/login", &c.to_string());
        assert_eq!(me.user.email, DEMO_EMAIL);
        assert_eq!(me.user.name, DEMO_NAME);
        assert!(me.user.is_super_admin);
        assert!(s.state().logged_in);
        let again: MeResponse = get(&mut s, "/v1/auth/me");
        assert_eq!(again, me);
    }
    // 본문이 JSON이 아니어도 (데모) 들어간다
    for body in [None, Some(""), Some("not json"), Some("{")] {
        let mut s = DemoServer::new(now());
        let (st, _) = s.handle(Method::Post, "/v1/auth/login", body);
        assert_eq!(st, 200);
    }
}

#[test]
fn me_requires_login_and_logout_clears() {
    let mut s = DemoServer::new(now());
    assert_eq!(err(&mut s, Method::Get, "/v1/auth/me", None).0, 401);
    // 다른 경로도 로그인 전에는 401 (로그인 화면으로 보내진다)
    assert_eq!(err(&mut s, Method::Get, "/v1/orgs", None).0, 401);
    assert_eq!(err(&mut s, Method::Get, &query::realtime(1), None).0, 401);
    login(&mut s);
    let me: MeResponse = get(&mut s, "/v1/auth/me");
    assert_eq!(me.orgs.len(), 1);
    assert_eq!(me.orgs[0].name, "바이너리큐브 데모");
    assert_eq!(me.orgs[0].role, Role::Admin);
    let names: Vec<i32> = me.orgs[0].projects.iter().map(|p| p.id).collect();
    assert_eq!(names, vec![1, 2]);
    // 로그아웃은 멱등 204
    for _ in 0..2 {
        let (st, body) = s.handle(Method::Post, "/v1/auth/logout", Some("{}"));
        assert_eq!((st, body.as_str()), (204, ""));
        assert!(!s.state().logged_in);
    }
    assert_eq!(err(&mut s, Method::Get, "/v1/auth/me", None).0, 401);
}

// ---------------------------------------------------------------- 모든 경로

#[test]
fn every_web_route_answers_with_its_dto() {
    let mut s = srv();
    let r = range(7);
    for pid in [1, 2] {
        let _: OverviewReport = get(&mut s, &query::report(pid, Report::Overview, r));
        let _: TimeseriesReport = get(&mut s, &query::report(pid, Report::Timeseries, r));
        let _: TimeseriesReport = get(
            &mut s,
            &query::report(pid, Report::Timeseries, r.previous()),
        );
        let _: Vec<ChannelTrafficRow> = get(&mut s, &query::report(pid, Report::Channels, r));
        let _: FunnelReport = get(&mut s, &query::report(pid, Report::Funnel, r));
        let _: RealtimeReport = get(&mut s, &query::realtime(pid));
        let _: Vec<PageRow> = get(&mut s, &query::pages(pid, r, "", 8));
        let _: Vec<PageRow> = get(&mut s, &query::pages(pid, r, "goods", 50));
        let events: Vec<EventCountRow> = get(&mut s, &query::events(pid, r, 100));
        for e in &events {
            for k in crate::pages::events::suggested_props(&e.event_name) {
                let _: Vec<PropertyValueRow> =
                    get(&mut s, &query::event_property(pid, &e.event_name, k, r, 20));
            }
        }
        let a: AttributionSettingsDto =
            get(&mut s, &format!("/v1/projects/{pid}/attribution-settings"));
        for lb in a.lookback_days.iter().map(|d| *d as u8) {
            for m in MODELS {
                let _: RevenueReport = get(&mut s, &query::revenue(pid, r, m, lb));
            }
            let _: OverlapReport = get(&mut s, &query::overlap(pid, r, lb, 8));
            let o: RecentOrdersReport = get(&mut s, &query::orders(pid, r, lb, 10, 0));
            let id = &o.rows[0].order_id;
            let _: OrderDetail = get(&mut s, &query::order(pid, id, lb));
        }
        let _: ProjectDto = get(&mut s, &format!("/v1/projects/{pid}"));
        let _: SnippetResponse = get(&mut s, &format!("/v1/projects/{pid}/snippet"));
        let _: ProjectSettingsResponse = get(&mut s, &format!("/v1/projects/{pid}/settings"));
        let _: OriginsResponse = get(&mut s, &format!("/v1/projects/{pid}/origins"));
        let _: Vec<ChannelRuleDto> = get(&mut s, &format!("/v1/projects/{pid}/channel-rules"));
        let _: Vec<ApiKeyDto> = get(&mut s, &format!("/v1/projects/{pid}/api-keys"));
        let _: AdSpendResponse = get(&mut s, &query::ad_spend(pid, r));
    }
    let _: MeResponse = get(&mut s, "/v1/auth/me");
    let orgs: Vec<OrgDto> = get(&mut s, "/v1/orgs");
    let id = orgs[0].id;
    let _: OrgDto = get(&mut s, &format!("/v1/orgs/{id}"));
    let _: Vec<MemberDto> = get(&mut s, &format!("/v1/orgs/{id}/members"));
    let _: Vec<ProjectDto> = get(&mut s, &format!("/v1/orgs/{id}/projects"));
    let _: Page<UserDto> = get(&mut s, &query::users("", 100, 0));
    let _: Page<UserDto> = get(&mut s, &query::users("데모", 100, 0));
}

#[test]
fn unknown_routes_are_404_json() {
    let mut s = srv();
    for p in [
        "/v1/nope",
        "/v2/auth/me",
        "/v1/projects/999/reports/overview",
        "/v1/projects/abc/settings",
        "/v1/orgs/999",
        "/v1/projects/1/reports/orders/NOPE-1",
        "/v1/projects/1/reports/unknown",
        "",
        "/",
    ] {
        assert_eq!(
            err(&mut s, Method::Get, p, None),
            (404, "not_found".into()),
            "{p}"
        );
    }
    assert_eq!(
        err(&mut s, Method::Delete, "/v1/auth/me", None),
        (404, "not_found".into())
    );
}

// ---------------------------------------------------------------- 리포트 일관성

#[test]
fn overview_matches_timeseries_and_channels() {
    let mut s = srv();
    for pid in [1, 2] {
        for days in [1, 7, 30, 90] {
            let r = range(days);
            let o = overview(&mut s, pid, r);
            assert_eq!(o.range.from, r.from.iso());
            assert_eq!(o.previous_range.to, r.from.add(-1).iso());
            let ts: TimeseriesReport = get(&mut s, &query::report(pid, Report::Timeseries, r));
            assert_eq!(ts.points.len(), days as usize);
            assert_eq!(ts.points.last().unwrap().date, today().iso());
            let pv: u64 = ts.points.iter().map(|p| p.pageviews).sum();
            let ss: u64 = ts.points.iter().map(|p| p.sessions).sum();
            assert_eq!(o.current.pageviews, pv);
            assert_eq!(o.current.sessions, ss);
            assert!(o.current.visitors <= ts.points.iter().map(|p| p.visitors).sum());
            assert!(o.current.visitors >= ts.points.iter().map(|p| p.visitors).max().unwrap());
            let ch: Vec<ChannelTrafficRow> = get(&mut s, &query::report(pid, Report::Channels, r));
            assert_eq!(
                ch.iter().map(|c| c.sessions).sum::<u64>(),
                o.current.sessions
            );
            assert_eq!(
                ch.iter().map(|c| c.pageviews).sum::<u64>(),
                o.current.pageviews
            );
            assert!(ch.windows(2).all(|w| w[0].sessions >= w[1].sessions));
            let pages: Vec<PageRow> = get(&mut s, &query::pages(pid, r, "", 1000));
            assert_eq!(
                pages.iter().map(|p| p.pageviews).sum::<u64>(),
                o.current.pageviews
            );
            assert_eq!(
                pages.iter().map(|p| p.entrances).sum::<u64>(),
                o.current.entrances
            );
            let ev: Vec<EventCountRow> = get(&mut s, &query::events(pid, r, 1000));
            assert_eq!(ev.iter().map(|e| e.count).sum::<u64>(), o.current.events);
            let page_view = ev.iter().find(|e| e.event_name == "page_view").unwrap();
            assert_eq!(page_view.count, o.current.pageviews);
            assert!(o.current.sessions > 0 && o.current.orders > 0 && o.current.revenue > 0);
            // 이전 기간 개요 = 같은 길이 앞 기간
            let prev = overview(&mut s, pid, r.previous());
            assert_eq!(prev.current, o.previous);
        }
    }
}

#[test]
fn channels_pages_events_are_realistic() {
    let mut s = srv();
    let r = range(30);
    let ch: Vec<ChannelTrafficRow> = get(&mut s, &query::report(1, Report::Channels, r));
    let ids: Vec<&str> = ch.iter().map(|c| c.channel.as_str()).collect();
    for c in [
        "naver_sa",
        "meta",
        "google_ads",
        "kakao",
        "criteo",
        "organic_search",
        "organic_social",
        "referral",
        "direct",
    ] {
        assert!(ids.contains(&c), "{c}");
    }
    let pages: Vec<PageRow> = get(&mut s, &query::pages(1, r, "", 50));
    let paths: Vec<&str> = pages.iter().map(|p| p.path.as_str()).collect();
    for p in [
        "/",
        "/goods/winter-padding",
        "/event/black-friday",
        "/cart",
        "/order/complete",
    ] {
        assert!(paths.contains(&p), "{p}");
    }
    assert!(pages.windows(2).all(|w| w[0].pageviews >= w[1].pageviews));
    // 경로 필터(부분 일치), limit
    let goods: Vec<PageRow> = get(&mut s, &query::pages(1, r, "/goods/", 50));
    assert!(!goods.is_empty() && goods.iter().all(|p| p.path.contains("/goods/")));
    let top3: Vec<PageRow> = get(&mut s, &query::pages(1, r, "", 3));
    assert_eq!(top3.len(), 3);
    let ev: Vec<EventCountRow> = get(&mut s, &query::events(1, r, 100));
    let names: Vec<&str> = ev.iter().map(|e| e.event_name.as_str()).collect();
    for e in [
        "page_view",
        "view_item",
        "add_to_cart",
        "begin_checkout",
        "purchase",
        "signup_complete",
        "coupon_download",
    ] {
        assert!(names.contains(&e), "{e}");
    }
    // 속성 분포: 합 = 이벤트 수, 내림차순
    for (e, k) in [
        ("view_item", "item_name"),
        ("purchase", "item_id"),
        ("coupon_download", "value"),
        ("signup_complete", "label"),
        ("page_view", "path"),
    ] {
        let count = ev.iter().find(|x| x.event_name == e).unwrap().count;
        let rows: Vec<PropertyValueRow> = get(&mut s, &query::event_property(1, e, k, r, 1000));
        assert!(!rows.is_empty(), "{e}.{k}");
        assert_eq!(rows.iter().map(|x| x.count).sum::<u64>(), count, "{e}.{k}");
        assert!(rows.windows(2).all(|w| w[0].count >= w[1].count));
    }
    // 모르는 이벤트·속성 → 빈 목록
    let none: Vec<PropertyValueRow> =
        get(&mut s, &query::event_property(1, "<script>", "x", r, 20));
    assert!(none.is_empty());
    let none: Vec<PropertyValueRow> = get(
        &mut s,
        &query::event_property(1, "view_item", "없는키", r, 20),
    );
    assert!(none.is_empty());
}

#[test]
fn funnel_is_monotonic() {
    let mut s = srv();
    for pid in [1, 2] {
        for days in [1, 7, 30, 120] {
            let f: FunnelReport = get(&mut s, &query::report(pid, Report::Funnel, range(days)));
            let names: Vec<&str> = f.steps.iter().map(|x| x.event_name.as_str()).collect();
            assert_eq!(
                names,
                ["view_item", "add_to_cart", "begin_checkout", "purchase"]
            );
            assert!(f.steps.windows(2).all(|w| w[0].visitors >= w[1].visitors));
            assert!(f.steps[3].visitors > 0);
            assert_eq!(f.steps[0].rate_from_prev, 1.0);
            assert_eq!(f.steps[0].rate_from_first, 1.0);
            for st in &f.steps {
                assert!((0.0..=1.0).contains(&st.rate_from_prev));
                assert!((0.0..=1.0).contains(&st.rate_from_first));
            }
        }
    }
}

#[test]
fn revenue_conserves_for_every_model_lookback_and_range() {
    let mut s = srv();
    let ranges = [
        range(1),
        range(7),
        range(30),
        range(120),
        // 데이터 이전 기간 (0) · 일부만 겹침
        DateRange {
            from: today().add(-400),
            to: today().add(-300),
        },
        DateRange {
            from: today().add(-150),
            to: today().add(-100),
        },
    ];
    for pid in [1, 2] {
        for r in ranges {
            let o = overview(&mut s, pid, r);
            for lb in [7, 30] {
                for m in MODELS {
                    let rev = revenue(&mut s, pid, r, m, lb);
                    let attributed: i64 = rev.channels.iter().map(|c| c.attributed).sum();
                    let claimed: i64 = rev.channels.iter().map(|c| c.raw_claimed).sum();
                    assert_eq!(attributed, rev.deduplicated, "{pid} {r:?} {m:?} {lb}");
                    assert_eq!(claimed, rev.raw_sum);
                    assert!(rev.raw_sum >= rev.deduplicated);
                    assert_eq!(rev.duplicate_amount, rev.raw_sum - rev.deduplicated);
                    // 실제 매출 = 개요 매출 (취소 제외)
                    assert_eq!(rev.deduplicated, o.current.revenue);
                    assert_eq!(rev.orders, o.current.orders);
                    assert_eq!(rev.model, m);
                    assert_eq!(rev.lookback_days, lb);
                    let spend: i64 = rev.channels.iter().map(|c| c.spend).sum();
                    assert_eq!(spend, rev.spend_total);
                    assert!(
                        rev.channels
                            .windows(2)
                            .all(|w| w[0].attributed >= w[1].attributed)
                    );
                    if rev.deduplicated > 0 {
                        let share: f64 = rev.channels.iter().map(|c| c.share).sum();
                        assert!((share - 1.0).abs() < 1e-9);
                    }
                }
            }
        }
    }
}

#[test]
fn duplicate_rate_and_roas_are_realistic() {
    let mut s = srv();
    for pid in [1, 2] {
        for lb in [7, 30] {
            let rev = revenue(&mut s, pid, range(30), Model::LastClick, lb);
            assert!(
                (0.25..=0.40).contains(&rev.duplicate_rate),
                "{pid} {lb} {}",
                rev.duplicate_rate
            );
            assert!(rev.spend_total > 0);
            let roas = rev.roas.unwrap();
            assert!((2.0..=12.0).contains(&roas), "{roas}");
            let assists: u64 = rev.channels.iter().map(|c| c.assists).sum();
            assert!(assists > 0);
            assert!(
                rev.channels
                    .iter()
                    .any(|c| c.channel == "naver_sa" && c.roas.is_some())
            );
        }
        // 긴 lookback은 접점을 더 많이 본다 → 중복이 더 크다
        let short = revenue(&mut s, pid, range(30), Model::LastClick, 7);
        let long = revenue(&mut s, pid, range(30), Model::LastClick, 30);
        assert!(long.raw_sum >= short.raw_sum);
        // 모델마다 채널 배분이 다르다
        let fc = revenue(&mut s, pid, range(30), Model::FirstClick, 30);
        assert_ne!(fc.channels, long.channels);
    }
}

#[test]
fn overlap_paths_have_two_or_more_channels() {
    let mut s = srv();
    let o: OverlapReport = get(&mut s, &query::overlap(1, range(30), 30, 8));
    assert!(o.multi_channel_orders > 0);
    assert!(!o.paths.is_empty() && o.paths.len() <= 8);
    assert!(o.paths.windows(2).all(|w| w[0].orders >= w[1].orders));
    for p in &o.paths {
        let mut d = p.channels.clone();
        d.sort();
        d.dedup();
        assert!(d.len() >= 2);
        assert!(p.channels.windows(2).all(|w| w[0] != w[1]));
    }
    assert!(o.paths.iter().map(|p| p.orders).sum::<u64>() <= o.multi_channel_orders);
}

#[test]
fn order_detail_shares_sum_to_amount() {
    let mut s = srv();
    for pid in [1, 2] {
        for lb in [7u8, 30] {
            let page: RecentOrdersReport = get(&mut s, &query::orders(pid, range(7), lb, 30, 0));
            assert!(!page.rows.is_empty());
            for row in &page.rows {
                let d: OrderDetail = get(&mut s, &query::order(pid, &row.order_id, lb));
                assert_eq!(d.order.order_id, row.order_id);
                assert_eq!(d.order.effective_amount, row.effective_amount);
                assert_eq!(d.lookback_days, lb);
                assert!(d.touches.windows(2).all(|w| w[0].ts <= w[1].ts));
                assert!(
                    d.touches
                        .iter()
                        .all(|t| t.ts.as_str() <= d.order.ordered_at.as_str())
                );
                if row.status == "CANCELLED" {
                    assert_eq!(row.last_click_channel, None);
                    assert!(d.shares.is_empty());
                    continue;
                }
                let models: Vec<Model> = d.shares.iter().map(|x| x.model).collect();
                assert_eq!(models, Model::ALL.to_vec());
                for sh in d.shares.iter().filter(|x| x.model.conserves_revenue()) {
                    let sum: i64 = sh.rows.iter().map(|x| x.amount).sum();
                    assert_eq!(
                        sum, d.order.effective_amount,
                        "{} {:?}",
                        row.order_id, sh.model
                    );
                }
                let lc = &d.shares[0].rows;
                let last = lc.iter().find(|x| x.amount > 0 || lc.len() == 1).unwrap();
                assert_eq!(
                    row.last_click_channel.as_deref(),
                    Some(last.channel.as_str())
                );
            }
        }
    }
}

#[test]
fn recent_orders_paging() {
    let mut s = srv();
    let r = range(30);
    let p1: RecentOrdersReport = get(&mut s, &query::orders(1, r, 7, 10, 0));
    let p2: RecentOrdersReport = get(&mut s, &query::orders(1, r, 7, 10, 10));
    assert_eq!(p1.rows.len(), 10);
    assert_eq!(p1.total, p2.total);
    assert!(p1.total > 100);
    assert!(
        p1.rows
            .iter()
            .all(|a| p2.rows.iter().all(|b| a.order_id != b.order_id))
    );
    let all: Vec<RecentOrderRow> = p1.rows.iter().chain(&p2.rows).cloned().collect();
    assert!(all.windows(2).all(|w| w[0].ordered_at >= w[1].ordered_at));
    // 마지막 쪽·범위 밖
    let last: RecentOrdersReport = get(&mut s, &query::orders(1, r, 7, 10, (p1.total - 3) as u32));
    assert_eq!(last.rows.len(), 3);
    let beyond: RecentOrdersReport = get(&mut s, &query::orders(1, r, 7, 10, p1.total as u32 + 5));
    assert!(beyond.rows.is_empty());
    // 취소 주문도 목록에는 나온다
    let big: RecentOrdersReport = get(&mut s, &query::orders(1, r, 7, 1000, 0));
    assert!(big.rows.iter().any(|x| x.status == "CANCELLED"));
    assert!(big.rows.iter().any(|x| x.status == "CONFIRMED"));
    // 미래의 주문은 없다
    assert!(big.rows[0].ordered_at.as_str() <= "2026-11-21T15:30:00+09:00");
}

#[test]
fn realtime_tracks_the_clock() {
    let mut s = srv();
    let rt: RealtimeReport = get(&mut s, &query::realtime(1));
    assert_eq!(rt.now, "2026-11-21T15:30:00+09:00");
    assert_eq!(rt.per_minute.len(), 30);
    assert_eq!(rt.per_minute.last().unwrap().minute, "15:30");
    assert_eq!(rt.per_minute[0].minute, "15:01");
    assert!(rt.active_5m > 0 && rt.active_5m <= rt.active_30m);
    assert!(!rt.top_pages.is_empty() && rt.top_pages.len() <= 10);
    let pv: u64 = rt.per_minute.iter().map(|m| m.pageviews).sum();
    assert!(rt.top_pages.iter().map(|p| p.pageviews).sum::<u64>() <= pv);
    // 1분 뒤에는 창이 1분 밀린다
    s.set_now(now() + 60_000);
    let next: RealtimeReport = get(&mut s, &query::realtime(1));
    assert_eq!(next.per_minute[0], rt.per_minute[1]);
    assert_eq!(next.per_minute.last().unwrap().minute, "15:31");
    // 새벽은 한산하다
    s.set_now(at(today(), 4, 0));
    let night: RealtimeReport = get(&mut s, &query::realtime(1));
    assert!(night.active_30m < rt.active_30m);
}

#[test]
fn today_grows_with_the_hour() {
    let r = range(1);
    let mut am = DemoServer::new(at(today(), 9, 0));
    let mut pm = DemoServer::new(at(today(), 18, 0));
    let mut end = DemoServer::new(at(today(), 23, 59));
    for s in [&mut am, &mut pm, &mut end] {
        login(s);
    }
    let (a, p, e) = (
        overview(&mut am, 1, r).current,
        overview(&mut pm, 1, r).current,
        overview(&mut end, 1, r).current,
    );
    assert!(a.sessions > 0 && a.sessions < p.sessions && p.sessions < e.sessions);
    assert!(a.orders <= p.orders && p.orders <= e.orders);
    // 자정 직후에는 거의 0
    let mut mid = DemoServer::new(at(today(), 0, 0));
    login(&mut mid);
    assert!(overview(&mut mid, 1, r).current.sessions < a.sessions);
    // 미래 기간은 0
    let future = DateRange {
        from: today().add(1),
        to: today().add(5),
    };
    let f = overview(&mut pm, 1, future).current;
    assert_eq!(f, OverviewMetrics::default());
}

#[test]
fn trend_seasonality_and_promo() {
    let mut s = srv();
    let ts: TimeseriesReport = get(&mut s, &query::report(1, Report::Timeseries, range(120)));
    let closed = &ts.points[..119];
    assert!(closed.iter().all(|p| p.sessions > 0));
    // 성장: 마지막 30일 평균 > 처음 30일 평균
    let avg = |v: &[TimeseriesPoint]| v.iter().map(|p| p.sessions).sum::<u64>() / v.len() as u64;
    assert!(avg(&closed[89..]) > avg(&closed[..30]));
    // 주말이 높다 (쇼핑몰)
    let (mut we, mut wd, mut nwe, mut nwd) = (0u64, 0u64, 0u64, 0u64);
    for p in &closed[..100] {
        let day = Day::parse(&p.date).unwrap();
        if day.weekday() >= 5 {
            we += p.sessions;
            nwe += 1;
        } else {
            wd += p.sessions;
            nwd += 1;
        }
    }
    assert!(we / nwe > wd / nwd);
    // 프로모션 급증: 최근 2주 안에 가장 큰 날이 있다
    let max = closed.iter().max_by_key(|p| p.sessions).unwrap();
    let day = Day::parse(&max.date).unwrap();
    assert!(today().0 - day.0 <= 14, "{}", max.date);
    // 120일보다 오래된 날은 0
    let old: TimeseriesReport = get(
        &mut s,
        &query::report(
            1,
            Report::Timeseries,
            DateRange {
                from: today().add(-200),
                to: today().add(-121),
            },
        ),
    );
    assert!(
        old.points
            .iter()
            .all(|p| p.sessions == 0 && p.pageviews == 0)
    );
}

#[test]
fn deterministic_and_projects_differ() {
    let paths = |pid| {
        vec![
            query::report(pid, Report::Overview, range(30)),
            query::report(pid, Report::Channels, range(7)),
            query::pages(pid, range(7), "", 50),
            query::events(pid, range(7), 100),
            query::realtime(pid),
            query::revenue(pid, range(30), Model::Linear, 30),
            query::overlap(pid, range(30), 7, 8),
            query::orders(pid, range(30), 7, 10, 20),
        ]
    };
    let (mut a, mut b) = (srv(), srv());
    for p in paths(1).into_iter().chain(paths(2)) {
        let x = a.handle(Method::Get, &p, None);
        let y = b.handle(Method::Get, &p, None);
        assert_eq!(x, y, "{p}");
        // 같은 요청 반복 (멱등)
        assert_eq!(a.handle(Method::Get, &p, None), x);
    }
    let o1 = overview(&mut a, 1, range(30)).current;
    let o2 = overview(&mut a, 2, range(30)).current;
    assert_ne!(o1, o2);
    assert!(o1.sessions > o2.sessions * 2, "쇼핑몰이 규모가 크다");
    assert!(
        o2.revenue / o2.orders as i64 > o1.revenue / o1.orders as i64,
        "브랜드몰 객단가가 높다"
    );
    // 다른 날(지난 날)의 숫자는 지금 시각과 무관하다
    let past = DateRange {
        from: today().add(-30),
        to: today().add(-1),
    };
    let mut later = DemoServer::new(now() + 5 * HOUR);
    login(&mut later);
    assert_eq!(overview(&mut a, 1, past), overview(&mut later, 1, past));
}

// ---------------------------------------------------------------- 입력 검증

#[test]
fn validation_errors_match_the_real_api() {
    let mut s = srv();
    let base = "/v1/projects/1/reports/overview";
    let bad = [
        format!("{base}?from=2026-11-21&to=2026-11-01"),
        format!("{base}?from=2025-01-01&to=2026-11-21"),
        format!("{base}?from=2026-13-01&to=2026-12-01"),
        format!("{base}?from=2026-11-01"),
        format!("{base}?to=2026-11-01"),
        format!("{base}?from=%3Cscript%3E&to=2026-11-01"),
        "/v1/projects/1/reports/revenue?model=raw_sum".into(),
        "/v1/projects/1/reports/revenue?model=foo".into(),
        "/v1/projects/1/reports/revenue?lookback=8".into(),
        "/v1/projects/1/reports/revenue?lookback=abc".into(),
        "/v1/projects/1/reports/orders?limit=0".into(),
        "/v1/projects/1/reports/orders?limit=1001".into(),
        "/v1/projects/1/reports/orders?offset=100001".into(),
        "/v1/projects/1/reports/orders?offset=-1".into(),
        format!("/v1/projects/1/reports/pages?q={}", "a".repeat(201)),
        format!("/v1/projects/1/reports/orders/{}", "x".repeat(200)),
    ];
    for p in &bad {
        assert_eq!(
            err(&mut s, Method::Get, p, None),
            (400, "invalid_input".into()),
            "{p}"
        );
    }
    // 경계: 400일, 1000건, 200자는 통과
    for ok in [
        format!(
            "{base}?from={}&to={}",
            today().add(-399).iso(),
            today().iso()
        ),
        "/v1/projects/1/reports/orders?limit=1000&offset=100000".into(),
        format!("/v1/projects/1/reports/pages?q={}", "a".repeat(200)),
        "/v1/projects/1/reports/overview".into(),
    ] {
        let (st, body) = s.handle(Method::Get, &ok, None);
        assert_eq!(st, 200, "{ok} {body}");
    }
    // 기간 없음 = 오늘로 끝나는 7일
    let o: OverviewReport = get(&mut s, base);
    assert_eq!(o.range.from, range(7).from.iso());
    assert_eq!(o.range.to, today().iso());
}

// ---------------------------------------------------------------- 설정·관리 변경

#[test]
fn settings_put_bumps_version_only_on_change() {
    let mut s = srv();
    let path = "/v1/projects/1/settings";
    let cur: ProjectSettingsResponse = get(&mut s, path);
    let same: ProjectSettingsResponse = send(
        &mut s,
        Method::Put,
        path,
        &serde_json::to_string(&cur.settings).unwrap(),
    );
    assert_eq!(same.settings_version, cur.settings_version);
    let mut next = cur.settings.clone();
    next.scroll_depth = !next.scroll_depth;
    let body = serde_json::to_string(&next).unwrap();
    let put: ProjectSettingsResponse = send(&mut s, Method::Put, path, &body);
    assert_eq!(put.settings_version, cur.settings_version + 1);
    assert_eq!(put.settings, next);
    let again: ProjectSettingsResponse = get(&mut s, path);
    assert_eq!(again, put);
    // 다시 같은 값 → 그대로 (멱등)
    let put2: ProjectSettingsResponse = send(&mut s, Method::Put, path, &body);
    assert_eq!(put2.settings_version, put.settings_version);
    // 다른 프로젝트는 영향 없음
    let other: ProjectSettingsResponse = get(&mut s, "/v1/projects/2/settings");
    assert_eq!(other.settings_version, cur.settings_version);
    // 잘못된 본문
    assert_eq!(err(&mut s, Method::Put, path, Some("nope")).0, 400);
    // 프로젝트 settings_version도 바뀐다
    let p: ProjectDto = get(&mut s, "/v1/projects/1");
    assert_eq!(p.settings_version, put.settings_version);
}

#[test]
fn origins_rules_attribution_keys_spend_mutate_state() {
    let mut s = srv();
    // 허용 도메인
    let o: OriginsResponse = send(
        &mut s,
        Method::Post,
        "/v1/projects/1/origins",
        r#"{"origin":"Shop.Example.COM"}"#,
    );
    assert!(o.origins.contains(&"shop.example.com".to_string()));
    assert_eq!(
        err(
            &mut s,
            Method::Post,
            "/v1/projects/1/origins",
            Some(r#"{"origin":"https://a/b"}"#)
        )
        .0,
        400
    );
    let (st, _) = s.handle(
        Method::Delete,
        &query::origin(1, "shop.example.com"),
        Some("{}"),
    );
    assert_eq!(st, 204);
    let o: OriginsResponse = get(&mut s, "/v1/projects/1/origins");
    assert!(!o.origins.contains(&"shop.example.com".to_string()));

    // 채널 규칙
    let before: Vec<ChannelRuleDto> = get(&mut s, "/v1/projects/1/channel-rules");
    let r: ChannelRuleDto = send(
        &mut s,
        Method::Post,
        "/v1/projects/1/channel-rules",
        r#"{"channel":"naver_shopping","match":{"utm_source":["naver_shop"]},"label":"쇼핑"}"#,
    );
    assert_eq!(r.priority, 100);
    let r2: ChannelRuleDto = send(
        &mut s,
        Method::Patch,
        &format!("/v1/projects/1/channel-rules/{}", r.id),
        r#"{"priority":5}"#,
    );
    assert_eq!(
        (r2.id, r2.priority, r2.channel.as_str()),
        (r.id, 5, "naver_shopping")
    );
    let list: Vec<ChannelRuleDto> = get(&mut s, "/v1/projects/1/channel-rules");
    assert_eq!(list.len(), before.len() + 1);
    assert_eq!(list[0].id, r.id, "우선순위 오름차순");
    assert_eq!(
        err(
            &mut s,
            Method::Post,
            "/v1/projects/1/channel-rules",
            Some(r#"{"channel":"x","match":{}}"#)
        )
        .0,
        400
    );
    let (st, _) = s.handle(
        Method::Delete,
        &format!("/v1/projects/1/channel-rules/{}", r.id),
        Some("{}"),
    );
    assert_eq!(st, 204);
    let list: Vec<ChannelRuleDto> = get(&mut s, "/v1/projects/1/channel-rules");
    assert_eq!(list, before);

    // 기여 설정: lookback 14 추가 → 리포트가 받는다
    let mut a: AttributionSettingsDto = get(&mut s, "/v1/projects/1/attribution-settings");
    assert_eq!(
        err(
            &mut s,
            Method::Get,
            &query::revenue(1, range(7), Model::Linear, 14),
            None
        )
        .0,
        400
    );
    a.lookback_days = vec![30, 14, 7, 14];
    a.default_model = Model::Linear;
    let saved: AttributionSettingsDto = send(
        &mut s,
        Method::Put,
        "/v1/projects/1/attribution-settings",
        &serde_json::to_string(&a).unwrap(),
    );
    assert_eq!(saved.lookback_days, vec![7, 14, 30]);
    let rev: RevenueReport = get(&mut s, &query::revenue(1, range(7), Model::Linear, 14));
    assert_eq!(rev.lookback_days, 14);
    let dflt: RevenueReport = get(&mut s, "/v1/projects/1/reports/revenue");
    assert_eq!((dflt.model, dflt.lookback_days), (Model::Linear, 7));
    for bad in [
        json!({"lookback_days": [0], "default_model": "linear", "half_life_hours": 168, "include_organic": true, "include_referral": true}),
        json!({"lookback_days": [7], "default_model": "raw_sum", "half_life_hours": 168, "include_organic": true, "include_referral": true}),
        json!({"lookback_days": [7], "default_model": "linear", "half_life_hours": 0, "include_organic": true, "include_referral": true}),
    ] {
        let (st, code) = err(
            &mut s,
            Method::Put,
            "/v1/projects/1/attribution-settings",
            Some(&bad.to_string()),
        );
        assert_eq!((st, code.as_str()), (400, "invalid_input"), "{bad}");
    }

    // 비밀 키: 평문은 생성 응답에만
    let created: CreatedApiKeyResponse =
        send(&mut s, Method::Post, "/v1/projects/1/api-keys", "{}");
    assert!(created.secret.starts_with("sk_demo_") && created.secret.len() > 20);
    assert!(created.key.prefix.starts_with("sk_demo_"));
    let keys: Vec<ApiKeyDto> = get(&mut s, "/v1/projects/1/api-keys");
    assert!(
        keys.iter()
            .any(|k| k.id == created.key.id && k.revoked_at.is_none())
    );
    let (_, listed) = s.handle(Method::Get, "/v1/projects/1/api-keys", None);
    assert!(!listed.contains(&created.secret));
    let (st, _) = s.handle(
        Method::Delete,
        &format!("/v1/projects/1/api-keys/{}", created.key.id),
        Some("{}"),
    );
    assert_eq!(st, 204);
    let keys: Vec<ApiKeyDto> = get(&mut s, "/v1/projects/1/api-keys");
    assert!(
        keys.iter()
            .any(|k| k.id == created.key.id && k.revoked_at.is_some())
    );

    // 광고비 CSV → 목록과 ROAS에 반영
    let day = today().add(-1).iso();
    let r1 = DateRange {
        from: today().add(-1),
        to: today().add(-1),
    };
    let before = revenue(&mut s, 1, r1, Model::LastClick, 7).spend_total;
    let csv = format!(
        "day,channel,campaign,amount\n{day},NAVER_SA,브랜드검색,1000000\n{day},email,뉴스레터,50000\n"
    );
    let up: AdSpendUploadResponse = {
        let (st, text) = s.handle(Method::Post, "/v1/projects/1/ad-spend", Some(&csv));
        assert_eq!(st, 200, "{text}");
        serde_json::from_str(&text).unwrap()
    };
    assert_eq!(up.rows, 2);
    let spend: AdSpendResponse = get(&mut s, &query::ad_spend(1, r1));
    assert!(
        spend.rows.iter().any(|r| r.channel == "naver_sa"
            && r.campaign == "브랜드검색"
            && r.amount == 1_000_000)
    );
    assert!(spend.rows.iter().any(|r| r.channel == "email"));
    let after = revenue(&mut s, 1, r1, Model::LastClick, 7);
    assert_ne!(after.spend_total, before);
    assert!(
        after
            .channels
            .iter()
            .any(|c| c.channel == "email" && c.spend == 50_000 && c.attributed == 0)
    );
    for bad in [
        "",
        "day,channel,campaign,amount\n",
        "2026-13-01,naver,,1\n",
        "2026-11-01,naver,,-1\n",
        "2026-11-01,naver\n",
    ] {
        let (st, code) = err(&mut s, Method::Post, "/v1/projects/1/ad-spend", Some(bad));
        assert_eq!((st, code.as_str()), (400, "invalid_input"), "{bad:?}");
    }
}

#[test]
fn admin_mutations() {
    let mut s = srv();
    // 광고주·프로젝트 생성 → me에 나타남
    let org: OrgDto = send(&mut s, Method::Post, "/v1/orgs", r#"{"name":"새 광고주"}"#);
    assert_eq!(org.role, Role::Admin);
    let p: ProjectDto = send(
        &mut s,
        Method::Post,
        &format!("/v1/orgs/{}/projects", org.id),
        r#"{"name":"새 몰"}"#,
    );
    assert_eq!(
        (p.org_id, p.timezone.as_str(), p.currency.as_str()),
        (org.id, "Asia/Seoul", "KRW")
    );
    assert!(p.id > 2);
    let me: MeResponse = get(&mut s, "/v1/auth/me");
    assert!(
        me.orgs
            .iter()
            .any(|o| o.id == org.id && o.projects.iter().any(|x| x.id == p.id))
    );
    // 새 프로젝트는 데이터가 없다 (0이지만 정상 응답)
    let o = overview(&mut s, p.id, range(7));
    assert_eq!(o.current, OverviewMetrics::default());
    let rt: RealtimeReport = get(&mut s, &query::realtime(p.id));
    assert_eq!(rt.active_30m, 0);
    assert_eq!(
        err(&mut s, Method::Post, "/v1/orgs", Some(r#"{"name":"  "}"#)).0,
        400
    );

    // 이름 변경, 일시 중지
    let up: OrgDto = send(
        &mut s,
        Method::Patch,
        &format!("/v1/orgs/{}", org.id),
        r#"{"memo":"메모"}"#,
    );
    assert_eq!((up.name.as_str(), up.memo.as_str()), ("새 광고주", "메모"));
    let paused: ProjectDto = send(
        &mut s,
        Method::Patch,
        &format!("/v1/projects/{}", p.id),
        r#"{"status":"paused"}"#,
    );
    assert_eq!(paused.status, ProjectStatus::Paused);

    // 사용자·멤버
    let u: UserDto = send(
        &mut s,
        Method::Post,
        "/v1/users",
        r#"{"email":"Kim@Example.com","password":"password-123","name":"김분석"}"#,
    );
    assert_eq!(u.email, "kim@example.com");
    assert_eq!(
        err(
            &mut s,
            Method::Post,
            "/v1/users",
            Some(r#"{"email":"kim@example.com","password":"password-123"}"#)
        )
        .0,
        409
    );
    assert_eq!(
        err(
            &mut s,
            Method::Post,
            "/v1/users",
            Some(r#"{"email":"bad","password":"password-123"}"#)
        )
        .0,
        400
    );
    let found: Page<UserDto> = get(&mut s, &query::users("kim", 100, 0));
    assert_eq!(found.total, 1);
    let m: MemberDto = send(
        &mut s,
        Method::Post,
        &format!("/v1/orgs/{}/members", org.id),
        r#"{"email":"kim@example.com","role":"analyst"}"#,
    );
    assert_eq!((m.user_id, m.role), (u.id, Role::Analyst));
    let ms: Vec<MemberDto> = get(&mut s, &format!("/v1/orgs/{}/members", org.id));
    assert!(ms.iter().any(|x| x.user_id == u.id));
    assert_eq!(
        err(
            &mut s,
            Method::Post,
            &format!("/v1/orgs/{}/members", org.id),
            Some(r#"{"email":"none@x.y","role":"viewer"}"#)
        ),
        (400, "user_not_found".into())
    );
    let (st, _) = s.handle(
        Method::Delete,
        &format!("/v1/orgs/{}/members/{}", org.id, u.id),
        Some("{}"),
    );
    assert_eq!(st, 204);
    let ms: Vec<MemberDto> = get(&mut s, &format!("/v1/orgs/{}/members", org.id));
    assert!(ms.iter().all(|x| x.user_id != u.id));
}

#[test]
fn state_survives_a_reload() {
    let mut s = srv();
    let _: OrgDto = send(
        &mut s,
        Method::Post,
        "/v1/orgs",
        r#"{"name":"남는 광고주"}"#,
    );
    let json = serde_json::to_string(s.state()).unwrap();
    let back: DemoState = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, s.state());
    let mut r = DemoServer::with_state(back, now());
    let orgs: Vec<OrgDto> = get(&mut r, "/v1/orgs");
    assert!(orgs.iter().any(|o| o.name == "남는 광고주"));
    // 저장값이 깨졌으면 기본 상태 (로그인 전)
    let broken = DemoState::decode(Some("{garbage"));
    assert_eq!(broken, DemoState::default());
    assert!(!broken.logged_in);
    assert_eq!(DemoState::decode(Some(&json)), *s.state());
    assert_eq!(DemoState::decode(None), DemoState::default());
}

#[test]
fn hostile_path_segments_are_safe() {
    let mut s = srv();
    let r = range(7);
    for evil in [
        "<script>alert(1)</script>",
        "../../etc/passwd",
        "'; DROP TABLE x; --",
        "%00",
        "🔥",
    ] {
        let rows: Vec<PageRow> = get(&mut s, &query::pages(1, r, evil, 50));
        assert!(rows.is_empty(), "{evil}");
        let (st, _) = s.handle(Method::Get, &query::order(1, evil, 7), None);
        assert_eq!(st, 404, "{evil}");
        let props: Vec<PropertyValueRow> =
            get(&mut s, &query::event_property(1, evil, evil, r, 20));
        assert!(props.is_empty());
    }
    // 아주 긴 경로·쿼리
    let long = format!("/v1/projects/1/reports/pages?q={}", "%41".repeat(10_000));
    assert_eq!(s.handle(Method::Get, &long, None).0, 400);
}

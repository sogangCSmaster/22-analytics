//! API 경로·쿼리 문자열 만들기 (순수 함수).
//!
//! 경로 조각(이벤트 이름, 주문번호, 도메인 등)과 쿼리 값은 모두 퍼센트 인코딩한다.

use super::dates::DateRange;
use analytics_core::attribution::Model;

/// 컴파일 시 `API_BASE`로 바꿀 수 있다.
#[cfg_attr(any(not(target_arch = "wasm32"), feature = "demo"), allow(dead_code))]
pub fn api_base() -> &'static str {
    option_env!("API_BASE").unwrap_or("http://localhost:8082")
}

/// RFC 3986 unreserved 이외의 바이트를 `%XX`로 인코딩 (UTF-8 기준).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 쿼리 문자열 빌더. 값이 빈 문자열인 선택 항목은 넣지 않는다.
#[derive(Default, Debug, Clone)]
pub struct Query {
    pairs: Vec<(String, String)>,
}

impl Query {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(mut self, k: &str, v: impl ToString) -> Self {
        self.pairs.push((k.to_owned(), v.to_string()));
        self
    }

    pub fn opt(self, k: &str, v: Option<impl ToString>) -> Self {
        match v.map(|v| v.to_string()) {
            Some(v) if !v.trim().is_empty() => self.push(k, v.trim()),
            _ => self,
        }
    }

    /// 기간 (`from`, `to`)
    pub fn range(self, r: DateRange) -> Self {
        self.push("from", r.from.iso()).push("to", r.to.iso())
    }

    /// `?a=1&b=2` (비었으면 빈 문자열)
    pub fn build(&self) -> String {
        if self.pairs.is_empty() {
            return String::new();
        }
        let body: Vec<String> = self
            .pairs
            .iter()
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect();
        format!("?{}", body.join("&"))
    }
}

/// `base` + `path` (중복 슬래시 없이). 브라우저 fetch에서만 쓴다.
#[cfg(any(all(target_arch = "wasm32", not(feature = "demo")), test))]
pub fn join(base: &str, path: &str) -> String {
    let b = base.trim_end_matches('/');
    let p = path.trim_start_matches('/');
    format!("{b}/{p}")
}

/// 배포 기본 경로 정규화: `22-analytics`, `/22-analytics/` → `/22-analytics`. 없거나 `/`면 빈 문자열.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn base_prefix(base: Option<&str>) -> String {
    match base.map(|b| b.trim_matches('/')) {
        Some(b) if !b.is_empty() => format!("/{b}"),
        _ => String::new(),
    }
}

/// 브라우저 주소에서 기본 경로를 뺀 앱 안의 경로 (`/22-analytics/p/1` → `/p/1`).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn strip_base(full: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return full.to_owned();
    }
    match full.strip_prefix(prefix) {
        Some("") => "/".into(),
        Some(rest) if rest.starts_with('/') => rest.to_owned(),
        Some(rest) if rest.starts_with('?') => format!("/{rest}"),
        _ => full.to_owned(),
    }
}

fn project(pid: i32) -> String {
    format!("/v1/projects/{pid}")
}

/// 단순 리포트 종류: 기간만 받는다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Report {
    Overview,
    Timeseries,
    Channels,
    Funnel,
}

impl Report {
    fn segment(self) -> &'static str {
        match self {
            Report::Overview => "overview",
            Report::Timeseries => "timeseries",
            Report::Channels => "channels",
            Report::Funnel => "funnel",
        }
    }
}

pub fn report(pid: i32, kind: Report, r: DateRange) -> String {
    format!(
        "{}/reports/{}{}",
        project(pid),
        kind.segment(),
        Query::new().range(r).build()
    )
}

pub fn realtime(pid: i32) -> String {
    format!("{}/reports/realtime", project(pid))
}

pub fn pages(pid: i32, r: DateRange, q: &str, limit: u32) -> String {
    format!(
        "{}/reports/pages{}",
        project(pid),
        Query::new()
            .range(r)
            .opt("q", Some(q))
            .push("limit", limit)
            .build()
    )
}

pub fn events(pid: i32, r: DateRange, limit: u32) -> String {
    format!(
        "{}/reports/events{}",
        project(pid),
        Query::new().range(r).push("limit", limit).build()
    )
}

pub fn event_property(pid: i32, event: &str, prop: &str, r: DateRange, limit: u32) -> String {
    format!(
        "{}/reports/events/{}/properties/{}{}",
        project(pid),
        encode(event),
        encode(prop),
        Query::new().range(r).push("limit", limit).build()
    )
}

pub fn revenue(pid: i32, r: DateRange, model: Model, lookback: u8) -> String {
    format!(
        "{}/reports/revenue{}",
        project(pid),
        Query::new()
            .range(r)
            .push("model", model.as_str())
            .push("lookback", lookback)
            .build()
    )
}

pub fn overlap(pid: i32, r: DateRange, lookback: u8, limit: u32) -> String {
    format!(
        "{}/reports/overlap{}",
        project(pid),
        Query::new()
            .range(r)
            .push("lookback", lookback)
            .push("limit", limit)
            .build()
    )
}

pub fn orders(pid: i32, r: DateRange, lookback: u8, limit: u32, offset: u32) -> String {
    format!(
        "{}/reports/orders{}",
        project(pid),
        Query::new()
            .range(r)
            .push("lookback", lookback)
            .push("limit", limit)
            .push("offset", offset)
            .build()
    )
}

pub fn order(pid: i32, order_id: &str, lookback: u8) -> String {
    format!(
        "{}/reports/orders/{}{}",
        project(pid),
        encode(order_id),
        Query::new().push("lookback", lookback).build()
    )
}

pub fn origin(pid: i32, origin: &str) -> String {
    format!("{}/origins/{}", project(pid), encode(origin))
}

pub fn ad_spend(pid: i32, r: DateRange) -> String {
    format!("{}/ad-spend{}", project(pid), Query::new().range(r).build())
}

pub fn users(q: &str, limit: u32, offset: u32) -> String {
    format!(
        "/v1/users{}",
        Query::new()
            .opt("q", Some(q.trim()))
            .push("limit", limit)
            .push("offset", offset)
            .build()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::dates::Day;

    fn r() -> DateRange {
        DateRange {
            from: Day::parse("2026-11-15").unwrap(),
            to: Day::parse("2026-11-21").unwrap(),
        }
    }

    #[test]
    fn encodes_reserved_and_unicode() {
        assert_eq!(encode("abc-_.~09"), "abc-_.~09");
        assert_eq!(encode("a b&c=d/e?f#g"), "a%20b%26c%3Dd%2Fe%3Ff%23g");
        assert_eq!(encode("구매"), "%EA%B5%AC%EB%A7%A4");
        assert_eq!(encode("<script>"), "%3Cscript%3E");
        assert_eq!(encode("%"), "%25");
        assert_eq!(encode(""), "");
    }

    #[test]
    fn query_builder() {
        assert_eq!(Query::new().build(), "");
        assert_eq!(
            Query::new().push("a", 1).push("b", "x y").build(),
            "?a=1&b=x%20y"
        );
        assert_eq!(
            Query::new().opt("q", Some("")).opt("n", None::<u8>).build(),
            ""
        );
        assert_eq!(Query::new().opt("q", Some("p")).build(), "?q=p");
    }

    #[test]
    fn joins_without_double_slash() {
        assert_eq!(join("http://a:8082/", "/v1/x"), "http://a:8082/v1/x");
        assert_eq!(join("http://a:8082", "v1/x"), "http://a:8082/v1/x");
        assert_eq!(join("", "/v1/x"), "/v1/x");
    }

    #[test]
    fn base_path_prefix() {
        assert_eq!(base_prefix(None), "");
        assert_eq!(base_prefix(Some("")), "");
        assert_eq!(base_prefix(Some("/")), "");
        assert_eq!(base_prefix(Some("22-analytics")), "/22-analytics");
        assert_eq!(base_prefix(Some("/22-analytics/")), "/22-analytics");
        assert_eq!(base_prefix(Some("a/b")), "/a/b");
    }

    #[test]
    fn strips_base_path() {
        let p = "/22-analytics";
        assert_eq!(
            strip_base("/22-analytics/p/1/pages?x=1", p),
            "/p/1/pages?x=1"
        );
        assert_eq!(strip_base("/22-analytics", p), "/");
        assert_eq!(strip_base("/22-analytics/", p), "/");
        assert_eq!(strip_base("/22-analytics?next=1", p), "/?next=1");
        // 이름만 비슷한 경로는 건드리지 않는다
        assert_eq!(strip_base("/22-analyticsX/p", p), "/22-analyticsX/p");
        assert_eq!(strip_base("/p/1", p), "/p/1");
        assert_eq!(strip_base("/p/1", ""), "/p/1");
    }

    #[test]
    fn report_paths() {
        assert_eq!(
            report(3, Report::Overview, r()),
            "/v1/projects/3/reports/overview?from=2026-11-15&to=2026-11-21"
        );
        assert_eq!(realtime(3), "/v1/projects/3/reports/realtime");
        assert_eq!(
            pages(3, r(), " /상품?id=1 ", 50),
            "/v1/projects/3/reports/pages?from=2026-11-15&to=2026-11-21&q=%2F%EC%83%81%ED%92%88%3Fid%3D1&limit=50"
        );
        assert_eq!(
            pages(3, r(), "  ", 50),
            "/v1/projects/3/reports/pages?from=2026-11-15&to=2026-11-21&limit=50"
        );
        assert_eq!(
            events(3, r(), 100),
            "/v1/projects/3/reports/events?from=2026-11-15&to=2026-11-21&limit=100"
        );
    }

    #[test]
    fn path_segments_are_encoded() {
        assert_eq!(
            event_property(3, "add/to cart", "item?id", r(), 20),
            "/v1/projects/3/reports/events/add%2Fto%20cart/properties/item%3Fid?from=2026-11-15&to=2026-11-21&limit=20"
        );
        assert_eq!(
            order(3, "ORD/1#2", 30),
            "/v1/projects/3/reports/orders/ORD%2F1%232?lookback=30"
        );
        assert_eq!(
            origin(3, "shop.client.com"),
            "/v1/projects/3/origins/shop.client.com"
        );
    }

    #[test]
    fn revenue_paths() {
        assert_eq!(
            revenue(3, r(), Model::TimeDecay, 7),
            "/v1/projects/3/reports/revenue?from=2026-11-15&to=2026-11-21&model=time_decay&lookback=7"
        );
        assert_eq!(
            overlap(3, r(), 30, 10),
            "/v1/projects/3/reports/overlap?from=2026-11-15&to=2026-11-21&lookback=30&limit=10"
        );
        assert_eq!(
            orders(3, r(), 7, 20, 40),
            "/v1/projects/3/reports/orders?from=2026-11-15&to=2026-11-21&lookback=7&limit=20&offset=40"
        );
        assert_eq!(
            ad_spend(3, r()),
            "/v1/projects/3/ad-spend?from=2026-11-15&to=2026-11-21"
        );
        assert_eq!(users(" kim ", 50, 0), "/v1/users?q=kim&limit=50&offset=0");
    }
}

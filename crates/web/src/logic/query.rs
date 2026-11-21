//! API 경로·쿼리 문자열 만들기 (순수 함수).
//!
//! 경로 조각(이벤트 이름, 주문번호, 도메인 등)과 쿼리 값은 모두 퍼센트 인코딩한다.

use super::dates::DateRange;
use analytics_core::attribution::Model;

/// 컴파일 시 `API_BASE`로 바꿀 수 있다.
pub fn api_base() -> &'static str {
    option_env!("API_BASE").unwrap_or("http://localhost:8082")
}

/// RFC 3986 unreserved 이외의 바이트를 `%XX`로 인코딩 (UTF-8 기준).
pub fn encode(s: &str) -> String {
    todo!()
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
        todo!()
    }

    /// 기간 (`from`, `to`)
    pub fn range(self, r: DateRange) -> Self {
        self.push("from", r.from.iso()).push("to", r.to.iso())
    }

    /// `?a=1&b=2` (비었으면 빈 문자열)
    pub fn build(&self) -> String {
        todo!()
    }
}

/// `base` + `path` (중복 슬래시 없이).
pub fn join(base: &str, path: &str) -> String {
    todo!()
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
    todo!()
}

pub fn events(pid: i32, r: DateRange, limit: u32) -> String {
    todo!()
}

pub fn event_property(pid: i32, event: &str, prop: &str, r: DateRange, limit: u32) -> String {
    todo!()
}

pub fn revenue(pid: i32, r: DateRange, model: Model, lookback: u8) -> String {
    todo!()
}

pub fn overlap(pid: i32, r: DateRange, lookback: u8, limit: u32) -> String {
    todo!()
}

pub fn orders(pid: i32, r: DateRange, lookback: u8, limit: u32, offset: u32) -> String {
    todo!()
}

pub fn order(pid: i32, order_id: &str, lookback: u8) -> String {
    todo!()
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
        assert_eq!(Query::new().push("a", 1).push("b", "x y").build(), "?a=1&b=x%20y");
        assert_eq!(Query::new().opt("q", Some("")).opt("n", None::<u8>).build(), "");
        assert_eq!(Query::new().opt("q", Some("p")).build(), "?q=p");
    }

    #[test]
    fn joins_without_double_slash() {
        assert_eq!(join("http://a:8082/", "/v1/x"), "http://a:8082/v1/x");
        assert_eq!(join("http://a:8082", "v1/x"), "http://a:8082/v1/x");
        assert_eq!(join("", "/v1/x"), "/v1/x");
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
        assert_eq!(origin(3, "shop.client.com"), "/v1/projects/3/origins/shop.client.com");
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

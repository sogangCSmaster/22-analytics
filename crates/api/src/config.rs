//! 환경 변수 설정.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// `API_BIND` (기본 `0.0.0.0:8082`. 대시보드 개발 서버가 8080을 쓴다)
    pub bind: String,
    /// `DATABASE_URL`
    pub database_url: String,
    /// `DASHBOARD_ORIGINS` (쉼표 구분). CORS와 CSRF Origin 검사에 쓴다.
    pub dashboard_origins: Vec<String>,
    /// `COOKIE_SECURE=true`면 세션 쿠키를 `__Host-at_session`(Secure, Path=/, Domain 없음)으로
    /// 발급한다. false(http 개발)면 `at_session`.
    pub cookie_secure: bool,
    /// `TRUST_PROXY_HEADERS=true`일 때만 `cf-connecting-ip`·`x-forwarded-for`를 클라이언트 IP로
    /// 믿는다 (Cloudflare·리버스 프록시 뒤 배포). 기본 false: 소켓 상대 주소를 쓴다.
    pub trust_proxy_headers: bool,
    /// `TRACKER_HOST`: 스니펫의 `t.js` 호스트
    pub tracker_host: String,
    pub login_limit: LoginLimitConfig,
    /// `PG_MAX_CONNECTIONS` (기본 [`DEFAULT_PG_MAX_CONNECTIONS`])
    pub pg_max_connections: u32,
}

/// api 서버 Postgres 풀 기본 크기.
pub const DEFAULT_PG_MAX_CONNECTIONS: u32 = 4;

/// 로그인 실패 제한. 창(window) 안의 실패 횟수가 상한에 닿으면 429.
#[derive(Debug, Clone)]
pub struct LoginLimitConfig {
    pub max_per_email: u32,
    pub max_per_ip: u32,
    /// 이메일이 잠긴 뒤에도 그 계정에 실패한 적 없는 IP에 허용하는 시도 수 (창 단위, 계정당).
    /// 공격자가 계정을 잠가 정상 사용자를 막는 DoS를 완화한다 ([`crate::ratelimit`] 참고).
    pub max_bypass_per_email: u32,
    pub window: Duration,
    /// 제한기에 들고 있는 키(이메일 해시·IP·조합) 수 상한.
    pub max_keys: usize,
}

impl Default for LoginLimitConfig {
    fn default() -> Self {
        Self {
            max_per_email: 5,
            max_per_ip: 20,
            max_bypass_per_email: 5,
            window: Duration::from_secs(15 * 60),
            max_keys: 20_000,
        }
    }
}

pub const DEFAULT_DASHBOARD_ORIGINS: &str = "http://localhost:8080,http://127.0.0.1:8080";

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8082".into(),
            database_url: "postgres://analytics:analytics@localhost:5432/analytics".into(),
            dashboard_origins: parse_origins(DEFAULT_DASHBOARD_ORIGINS),
            cookie_secure: false,
            trust_proxy_headers: false,
            tracker_host: "http://localhost:8081".into(),
            login_limit: LoginLimitConfig::default(),
            pg_max_connections: DEFAULT_PG_MAX_CONNECTIONS,
        }
    }
}

impl ApiConfig {
    pub fn from_env() -> Self {
        let d = Self::default();
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Self {
            bind: get("API_BIND").unwrap_or(d.bind),
            database_url: get("DATABASE_URL").unwrap_or(d.database_url),
            dashboard_origins: get("DASHBOARD_ORIGINS")
                .map(|v| parse_origins(&v))
                .unwrap_or(d.dashboard_origins),
            cookie_secure: get("COOKIE_SECURE").is_some_and(|v| parse_bool(&v)),
            trust_proxy_headers: get("TRUST_PROXY_HEADERS").is_some_and(|v| parse_bool(&v)),
            tracker_host: get("TRACKER_HOST").unwrap_or(d.tracker_host),
            login_limit: d.login_limit,
            pg_max_connections: analytics_pg::max_connections_from_env(d.pg_max_connections),
        }
    }
}

fn parse_bool(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// `"a, b,,c/"` → `["a", "b", "c"]` (끝 슬래시 제거).
pub fn parse_origins(s: &str) -> Vec<String> {
    s.split(',')
        .map(|o| o.trim().trim_end_matches('/').to_owned())
        .filter(|o| !o.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_pool_default_is_small() {
        // 요청 핸들러 수에 비해 PG 쿼리는 짧다: 4개면 충분하고 DB 연결 수를 아낀다
        assert_eq!(ApiConfig::default().pg_max_connections, 4);
    }

    #[test]
    fn parses_origin_list() {
        assert_eq!(
            parse_origins(" https://a.test/, ,http://b.test:8080"),
            vec!["https://a.test", "http://b.test:8080"]
        );
        assert_eq!(
            ApiConfig::default().dashboard_origins,
            vec!["http://localhost:8080", "http://127.0.0.1:8080"]
        );
    }

    #[test]
    fn default_bind_does_not_clash_with_dashboard_dev_server() {
        // 대시보드 개발 서버가 8080을 쓴다
        assert_eq!(ApiConfig::default().bind, "0.0.0.0:8082");
    }

    #[test]
    fn proxy_headers_are_untrusted_by_default() {
        assert!(!ApiConfig::default().trust_proxy_headers);
    }

    #[test]
    fn parses_bool_flags() {
        assert!(parse_bool("TRUE") && parse_bool("1") && parse_bool(" yes "));
        assert!(!parse_bool("false") && !parse_bool("0") && !parse_bool("nope"));
    }
}

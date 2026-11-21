//! 환경 변수 설정.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// `API_BIND`
    pub bind: String,
    /// `DATABASE_URL`
    pub database_url: String,
    /// `DASHBOARD_ORIGINS` (쉼표 구분). CORS와 CSRF Origin 검사에 쓴다.
    pub dashboard_origins: Vec<String>,
    /// `COOKIE_SECURE=true`면 세션 쿠키에 `Secure`를 붙인다.
    pub cookie_secure: bool,
    /// `TRACKER_HOST`: 스니펫의 `t.js` 호스트
    pub tracker_host: String,
    pub login_limit: LoginLimitConfig,
}

/// 로그인 실패 제한. 창(window) 안의 실패 횟수가 상한에 닿으면 429.
#[derive(Debug, Clone)]
pub struct LoginLimitConfig {
    pub max_per_email: u32,
    pub max_per_ip: u32,
    pub window: Duration,
}

impl Default for LoginLimitConfig {
    fn default() -> Self {
        Self {
            max_per_email: 5,
            max_per_ip: 20,
            window: Duration::from_secs(15 * 60),
        }
    }
}

pub const DEFAULT_DASHBOARD_ORIGINS: &str = "http://localhost:8080,http://127.0.0.1:8080";

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8080".into(),
            database_url: "postgres://analytics:analytics@localhost:5432/analytics".into(),
            dashboard_origins: parse_origins(DEFAULT_DASHBOARD_ORIGINS),
            cookie_secure: false,
            tracker_host: "http://localhost:8081".into(),
            login_limit: LoginLimitConfig::default(),
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
            tracker_host: get("TRACKER_HOST").unwrap_or(d.tracker_host),
            login_limit: d.login_limit,
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
    fn parses_bool_flags() {
        assert!(parse_bool("TRUE") && parse_bool("1") && parse_bool(" yes "));
        assert!(!parse_bool("false") && !parse_bool("0") && !parse_bool("nope"));
    }
}

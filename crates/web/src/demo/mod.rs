//! 데모 모드: 브라우저 안의 가짜 API 서버 (`--features demo`).
//!
//! - 모든 API 호출은 [`crate::api`]의 `raw()`에서 [`DemoServer::handle`]로 바뀐다.
//! - 응답은 실제 API 계약(`analytics_core::dto`) 그대로의 JSON이다.
//! - 순수 Rust(web-sys 없음): `cargo test -p analytics-web --no-default-features`로 검증한다.

use crate::api::Method;
use serde::{Deserialize, Serialize};

/// 데모 사용자
pub const DEMO_EMAIL: &str = "demo@binarycube.co.kr";
pub const DEMO_NAME: &str = "데모 관리자";
/// localStorage 키 (로그인 여부 + 변경한 설정)
pub const STORAGE_KEY: &str = "at_demo_state";

/// 데모 서버의 바뀌는 상태 (로그인, 설정 변경). 리포트 숫자는 상태가 아니라 계산 결과다.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DemoState {
    pub logged_in: bool,
}

impl DemoState {
    /// localStorage 값 → 상태. 없거나 깨졌으면 기본값.
    pub fn decode(_s: Option<&str>) -> Self {
        Self::default()
    }
}

pub struct DemoServer {
    state: DemoState,
    now_ms: i64,
}

impl DemoServer {
    pub fn new(now_ms: i64) -> Self {
        Self::with_state(DemoState::default(), now_ms)
    }

    pub fn with_state(state: DemoState, now_ms: i64) -> Self {
        Self { state, now_ms }
    }

    pub fn set_now(&mut self, now_ms: i64) {
        self.now_ms = now_ms;
    }

    pub fn state(&self) -> &DemoState {
        &self.state
    }

    /// (상태 코드, 본문). 204는 빈 본문.
    pub fn handle(&mut self, _method: Method, _path: &str, _body: Option<&str>) -> (u16, String) {
        let _ = self.now_ms;
        (
            404,
            r#"{"error":"not_found","message":"찾을 수 없습니다"}"#.into(),
        )
    }
}

#[cfg(test)]
mod tests;

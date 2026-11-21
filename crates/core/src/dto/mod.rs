//! API ↔ 대시보드 공유 DTO. JSON 직렬화 형식이 곧 API 계약이다.
//! 날짜는 `YYYY-MM-DD` 문자열(프로젝트 타임존 기준), 금액은 통화 최소단위 정수.

pub mod admin;
pub mod reports;

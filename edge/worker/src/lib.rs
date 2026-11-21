//! Cloudflare Worker 엣지.
//!
//! 판단 로직은 모두 플랫폼 독립 모듈에 두고 호스트에서 `cargo test -p analytics-edge`로 검증한다.
//! `worker` 크레이트에 닿는 코드는 wasm32 전용 `glue` 모듈 하나뿐이다.
//!
//! | 모듈 | 역할 |
//! |---|---|
//! | `http` | 라우팅, 응답 구조체, Content-Length 검사 |
//! | `collect` | `/v1/collect` 검증·보강 (core `collect::process`) → 응답 + 전달할 이벤트 |
//! | `config` | KV 키, 5분 메모리 캐시, `/v1/config` 응답 |
//! | `orders` | `/v1/orders(:batch)` 프록시 검증·응답 매핑 |
//! | `cookie` | `_at_vid` 파싱·발급, 쿠키 Domain |
//! | `cors` | Origin 검증, preflight |
//! | `forward` | 원점 서명 요청, 결과 분류, Queue 메시지·재시도 정책 |

pub mod collect;
pub mod config;
pub mod cookie;
pub mod cors;
pub mod forward;
pub mod http;
pub mod orders;

#[cfg(target_arch = "wasm32")]
mod glue;

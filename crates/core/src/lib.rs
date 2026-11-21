//! 22-analytics 공유 도메인 로직.
//!
//! 이 크레이트는 I/O가 없는 순수 로직만 담는다. Cloudflare Worker(wasm), ingest 서버의
//! 엣지 에뮬레이션, 기여 연산 잡이 모두 같은 코드를 쓰도록 하기 위해서다.
//! 난수·시계가 필요한 곳은 호출자가 값을 주입한다.

pub mod attribution;
pub mod cache;
pub mod channel;
pub mod collect;
pub mod dto;
pub mod edge_auth;
pub mod event;
pub mod ids;
pub mod orders;
pub mod settings;
pub mod timeutil;
pub mod urlx;

pub use event::Event;
pub use ids::{ProjectId, PublicKey};

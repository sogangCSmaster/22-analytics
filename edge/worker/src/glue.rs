//! wasm32 전용 계층: `worker` 크레이트 타입 ↔ 플랫폼 독립 로직.
//!
//! 이 파일에는 판단 로직을 두지 않는다. 헤더·바인딩을 읽어 순수 함수에 넘기고, 결과를
//! `fetch`·KV·Queue 호출로 옮길 뿐이다. 판단은 모두 네이티브 테스트가 덮는 모듈에 있다.

use crate::collect::{self, CollectRequest};
use crate::config::{self, ConfigCache};
use crate::cors;
use crate::forward::{self, Delivery, DropReason, ForwardRequest, QueueAction, QueuedForward};
use crate::http::{self, EdgeResponse, OrdersKind, Route};
use crate::orders::{self, OrdersRequest, OriginReply};
use analytics_core::collect::ProjectConfig;
use analytics_core::event::MAX_BATCH_BYTES;
use analytics_core::{Event, PublicKey};
use futures_util::future::{Either, select};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use uuid::Uuid;
use wasm_bindgen::prelude::wasm_bindgen;
use worker::{
    AbortController, Context, Date, Delay, Env, Fetch, Headers, MessageBatch, MessageExt, Method,
    QueueRetryOptionsBuilder, Request, RequestInit, Response, console_error, console_warn, event,
    js_sys,
};

const KV_BINDING: &str = "PROJECTS";
const QUEUE_BINDING: &str = "RETRY_QUEUE";
const VAR_ORIGIN_URL: &str = "ORIGIN_URL";
const SECRET_EDGE: &str = "EDGE_SHARED_SECRET";

/// 수집 전달은 `waitUntil` 안에서 돈다 (브라우저 응답과 무관). 넘기면 Queue로.
const COLLECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 주문은 가맹점 서버가 응답을 기다린다.
const ORDERS_TIMEOUT: Duration = Duration::from_secs(20);
const QUEUE_FORWARD_TIMEOUT: Duration = Duration::from_secs(15);

thread_local! {
    /// isolate 단위 메모리 캐시 (ARCHITECTURE §4.1: 5분)
    static CACHE: RefCell<ConfigCache> = RefCell::new(config::new_cache());
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = crypto, js_name = randomUUID)]
    fn random_uuid() -> String;
}

fn new_uuid() -> Uuid {
    Uuid::parse_str(&random_uuid()).unwrap_or_else(|_| {
        // crypto.randomUUID는 항상 유효한 값을 준다. 만일을 위한 대체 경로.
        let hi = (js_sys::Math::random() * u64::MAX as f64) as u64;
        let lo = (js_sys::Math::random() * u64::MAX as f64) as u64;
        uuid::Builder::from_random_bytes(((u128::from(hi) << 64) | u128::from(lo)).to_be_bytes())
            .into_uuid()
    })
}

fn now_ms() -> i64 {
    Date::now().as_millis() as i64
}

fn header(req: &Request, name: &str) -> Option<String> {
    req.headers().get(name).ok().flatten()
}

struct Origin {
    url: String,
    secret: Vec<u8>,
}

fn origin(env: &Env) -> Option<Origin> {
    let url = env.var(VAR_ORIGIN_URL).ok()?.to_string();
    let secret = env.secret(SECRET_EDGE).ok()?.to_string();
    if url.is_empty() || !forward::valid_secret(&secret) {
        console_error!("ORIGIN_URL 또는 EDGE_SHARED_SECRET(32자 이상) 설정 누락");
        return None;
    }
    Some(Origin {
        url,
        secret: secret.into_bytes(),
    })
}

// ───────────────────────── fetch ─────────────────────────

#[event(fetch)]
pub async fn fetch(mut req: Request, env: Env, ctx: Context) -> worker::Result<Response> {
    let method = req.method().to_string();
    let r = match http::route(&method, &req.path()) {
        Route::Collect => handle_collect(&mut req, &env, &ctx).await,
        Route::Config => handle_config(&req, &env).await,
        Route::Orders(kind) => handle_orders(&mut req, &env, kind).await,
        Route::Preflight => cors::preflight(header(&req, "origin").as_deref()),
        Route::MethodNotAllowed => EdgeResponse::empty(405),
        Route::NotFound => EdgeResponse::empty(404),
    };
    to_response(r)
}

fn to_response(r: EdgeResponse) -> worker::Result<Response> {
    let headers = Headers::new();
    for (k, v) in &r.headers {
        headers.append(k, v)?;
    }
    // 204 등 본문 없는 상태 코드에 빈 본문을 붙이면 런타임이 거부하므로 empty()를 쓴다.
    let resp = if r.body.is_empty() {
        Response::empty()?
    } else {
        Response::from_bytes(r.body)?
    };
    Ok(resp.with_status(r.status).with_headers(headers))
}

async fn project(env: &Env, key: &PublicKey) -> worker::Result<Option<Rc<ProjectConfig>>> {
    let now = now_ms();
    if let Some(hit) = CACHE.with(|c| c.borrow().get(key.as_str(), now)) {
        return Ok(hit);
    }
    let cfg = env
        .kv(KV_BINDING)?
        .get(&config::kv_key(key))
        .json::<ProjectConfig>()
        .await?
        .map(Rc::new);
    CACHE.with(|c| c.borrow_mut().put(key.to_string(), cfg.clone(), now));
    Ok(cfg)
}

async fn handle_collect(req: &mut Request, env: &Env, ctx: &Context) -> EdgeResponse {
    if http::declared_too_large(header(req, "content-length").as_deref(), MAX_BATCH_BYTES) {
        return EdgeResponse::empty(413);
    }
    let Ok(body) = req.bytes().await else {
        return EdgeResponse::empty(204);
    };
    let (key, batch) = match collect::parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let cfg = match project(env, &key).await {
        Ok(c) => c,
        Err(e) => {
            console_error!("프로젝트 설정 조회 실패: {e}");
            return EdgeResponse::empty(204);
        }
    };

    let host = req
        .url()
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
    let (origin_h, referer, ua, cookie, dnt) = (
        header(req, "origin"),
        header(req, "referer"),
        header(req, "user-agent").unwrap_or_default(),
        header(req, "cookie"),
        header(req, "dnt"),
    );
    let cf = req.cf();
    let country = cf.and_then(|c| c.country()).unwrap_or_default();
    let region = cf.and_then(|c| c.region()).unwrap_or_default();
    let city = cf.and_then(|c| c.city()).unwrap_or_default();

    let creq = CollectRequest {
        host: &host,
        origin: origin_h.as_deref(),
        referer: referer.as_deref(),
        user_agent: &ua,
        cookie: cookie.as_deref(),
        dnt: dnt.as_deref() == Some("1"),
        country: &country,
        region: &region,
        city: &city,
        now_ms: now_ms(),
    };
    let out = collect::handle(&creq, cfg.as_deref(), batch, new_uuid);
    if !out.events.is_empty() {
        let env = env.clone();
        let events = out.events;
        ctx.wait_until(async move { deliver(env, events).await });
    }
    out.response
}

/// 원점 전달 → 실패하면 Queue. 브라우저 응답 뒤(`waitUntil`)에 돈다.
async fn deliver(env: Env, events: Vec<Event>) {
    let status = match origin(&env) {
        Some(o) => {
            let body = forward::collect_body(&events).into_bytes();
            let f = forward::signed(&o.url, forward::COLLECT_PATH, &o.secret, body, now_ms());
            send(&f, COLLECT_TIMEOUT).await.map(|r| r.status_code())
        }
        None => None,
    };
    match forward::classify(status) {
        Delivery::Delivered => {}
        Delivery::Rejected(s) => {
            console_error!(
                "원점이 수집 배치를 거절 (status {s}), 이벤트 {}건 폐기",
                events.len()
            );
        }
        Delivery::Retry => enqueue(&env, &events).await,
    }
}

async fn enqueue(env: &Env, events: &[Event]) {
    let queue = match env.queue(QUEUE_BINDING) {
        Ok(q) => q,
        Err(e) => {
            console_error!("Queue 바인딩 없음, 이벤트 {}건 유실: {e}", events.len());
            return;
        }
    };
    for m in forward::queue_messages(events) {
        if let Err(e) = queue.send(m).await {
            console_error!("Queue 전송 실패, 배치 유실: {e}");
        }
    }
}

/// 서명된 요청을 보낸다. 네트워크 오류·타임아웃은 `None`.
async fn send(f: &ForwardRequest, timeout: Duration) -> Option<Response> {
    let headers = Headers::new();
    for (k, v) in &f.headers {
        headers.set(k, v).ok()?;
    }
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(js_sys::Uint8Array::from(f.body.as_slice()).into()));
    let request = Request::new_with_init(&f.url, &init).ok()?;
    let controller = AbortController::default();
    let signal = controller.signal();
    let fetch = Fetch::Request(request);
    let sending = Box::pin(fetch.send_with_signal(&signal));
    let timer = Box::pin(Delay::from(timeout));
    match select(sending, timer).await {
        Either::Left((Ok(resp), _)) => Some(resp),
        Either::Left((Err(e), _)) => {
            console_warn!("원점 요청 실패: {e}");
            None
        }
        Either::Right((_, pending)) => {
            drop(pending);
            controller.abort();
            console_warn!("원점 요청 타임아웃 ({}s)", timeout.as_secs());
            None
        }
    }
}

async fn handle_config(req: &Request, env: &Env) -> EdgeResponse {
    let query = req.url().ok().and_then(|u| u.query().map(str::to_owned));
    let Some(key) = config::key_from_query(query.as_deref()) else {
        return config::config_response(None);
    };
    match project(env, &key).await {
        Ok(cfg) => config::config_response(cfg.as_deref()),
        Err(e) => {
            console_error!("프로젝트 설정 조회 실패: {e}");
            EdgeResponse::empty(503)
        }
    }
}

async fn handle_orders(req: &mut Request, env: &Env, kind: OrdersKind) -> EdgeResponse {
    if http::declared_too_large(header(req, "content-length").as_deref(), kind.body_limit()) {
        return EdgeResponse::api_error(http::ApiError::TooLarge);
    }
    let Ok(body) = req.bytes().await else {
        return EdgeResponse::error(400, "bad_body");
    };
    let Some(o) = origin(env) else {
        return orders::map_response(None);
    };
    let (authorization, idempotency_key) =
        (header(req, "authorization"), header(req, "idempotency-key"));
    let oreq = OrdersRequest {
        kind,
        authorization: authorization.as_deref(),
        idempotency_key: idempotency_key.as_deref(),
        body: &body,
    };
    let f = match orders::build_forward(&oreq, &o.url, &o.secret, now_ms()) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let reply = match send(&f, ORDERS_TIMEOUT).await {
        Some(mut resp) => {
            let content_type = resp.headers().get("content-type").ok().flatten();
            match resp.bytes().await {
                Ok(body) => Some(OriginReply {
                    status: resp.status_code(),
                    content_type,
                    body,
                }),
                Err(_) => None,
            }
        }
        None => None,
    };
    orders::map_response(reply)
}

// ───────────────────────── queue consumer ─────────────────────────

#[event(queue)]
pub async fn queue(
    batch: MessageBatch<QueuedForward>,
    env: Env,
    _ctx: Context,
) -> worker::Result<()> {
    let policy = forward::RetryPolicy::default();
    let o = origin(&env);
    for msg in batch.raw_iter() {
        let parsed = js_sys::JSON::stringify(&msg.body())
            .ok()
            .and_then(|s| s.as_string())
            .and_then(|s| serde_json::from_str::<QueuedForward>(&s).ok())
            .filter(|m| m.v == forward::QUEUE_MESSAGE_VERSION);
        let Some(m) = parsed else {
            console_error!("해석할 수 없는 Queue 메시지 폐기: {}", msg.id());
            msg.ack();
            continue;
        };
        let status = match &o {
            Some(o) => {
                let f = m.to_request(&o.url, &o.secret, now_ms());
                send(&f, QUEUE_FORWARD_TIMEOUT)
                    .await
                    .map(|r| r.status_code())
            }
            None => None,
        };
        match policy.decide(forward::classify(status), msg.attempts()) {
            QueueAction::Ack => msg.ack(),
            QueueAction::Retry { delay_secs } => msg.retry_with_options(
                &QueueRetryOptionsBuilder::new()
                    .with_delay_seconds(delay_secs)
                    .build(),
            ),
            QueueAction::Drop(reason) => {
                match reason {
                    DropReason::Rejected(s) => {
                        console_error!("원점이 재전송 배치를 거절 (status {s}), 폐기: {}", msg.id())
                    }
                    DropReason::Exhausted => {
                        console_error!("재시도 {}회 초과, 배치 폐기: {}", msg.attempts(), msg.id())
                    }
                }
                msg.ack();
            }
        }
    }
    Ok(())
}

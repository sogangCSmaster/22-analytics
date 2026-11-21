# analytics-edge (Cloudflare Worker)

workers-rs(Rust → WASM)로 만든 수집 엣지입니다. 동작 명세는 `docs/ARCHITECTURE.md` §1, §3.4, §4, 로컬 대응은 §16(ingest 엣지 에뮬레이션, `crates/ingest/src/emulate.rs`)을 따릅니다.

> 아직 배포하지 않습니다. Cloudflare 계정·도메인을 받은 뒤 아래 절차로 올립니다.

## 엔드포인트

| 경로 | 동작 |
|---|---|
| `GET /t.js` 등 정적 파일 | wrangler `[assets]`(`tracker/dist`)가 Worker 앞에서 응답 |
| `POST /v1/collect` | 키·Origin 검증, 봇 필터, geo(`request.cf`) 보강, `_at_vid` 쿠키 발급 → 즉시 `204`, `waitUntil`로 `{ORIGIN_URL}/internal/collect`에 HMAC 서명 전달. 실패(네트워크·타임아웃·5xx·408·429)하면 `RETRY_QUEUE`에 넣는다. 거부 사유는 노출하지 않는다(항상 `204`, 64KB 초과만 `413`). |
| `OPTIONS /v1/collect`, `/v1/config` | CORS preflight `204` |
| `GET /v1/config?k=pk_...` | KV `project:{pk}` → 공개 설정 JSON, `Cache-Control: public, max-age=300`. 없거나 비활성이면 `404` |
| `POST /v1/orders`, `/v1/orders:batch` | 본문을 바이트 그대로 서명해 `{ORIGIN_URL}/internal/orders`로 전달 (`x-forwarded-authorization`, `x-forwarded-path`, 선택 `idempotency-key`). 원점 상태·본문을 그대로 돌려준다. 64KB / 1MB 초과는 `413`, `Authorization: Bearer ...`가 없으면 엣지에서 `401`. 원점에 닿지 못하면 `503` + `Retry-After` (Queue에 넣지 않음, 가맹점 서버가 재시도) |
| Queue consumer | 재전송 배치를 새 타임스탬프로 다시 서명해 전달. 2xx·4xx는 ack, 그 밖은 30초부터 2배씩(최대 1시간) 재시도, 10회 넘으면 로그 남기고 폐기 |

## 코드 구조

판단 로직은 플랫폼 독립 모듈에 있고 호스트에서 테스트합니다. `worker` 크레이트는 `src/glue.rs`(wasm32 전용)에서만 씁니다.

| 파일 | 역할 |
|---|---|
| `src/http.rs` | 라우팅, 응답 구조체, `Content-Length` 검사 |
| `src/collect.rs` | `/v1/collect` 판단 (`analytics_core::collect::process` 호출) |
| `src/config.rs` | KV 키, 60초 메모리 캐시, `/v1/config` 응답 |
| `src/orders.rs` | 주문 프록시 검증·응답 매핑 |
| `src/cookie.rs` | `_at_vid` 파싱·발급, 쿠키 `Domain`(등록 가능 도메인, IP·localhost·workers.dev는 생략) |
| `src/cors.rs` | Origin 검증, preflight |
| `src/forward.rs` | 서명 요청, 결과 분류, Queue 메시지 분할(100KB), 재시도 정책 |
| `src/glue.rs` | `#[event(fetch)]`, `#[event(queue)]`, KV·Queue·fetch 호출 |

## 테스트·빌드

```bash
cargo test -p analytics-edge                                           # 네이티브 단위 테스트
cargo build -p analytics-edge --target wasm32-unknown-unknown --release
cargo clippy -p analytics-edge --target wasm32-unknown-unknown -- -D warnings
```

## 배포 (계정 수령 후)

1. 트래커 빌드: `npm run build --prefix tracker` (`[assets]`가 `tracker/dist`를 올린다)
2. KV 생성 후 `wrangler.toml`의 `id` 교체
   ```bash
   cd edge/worker
   npx wrangler kv namespace create PROJECTS
   ```
3. Queue 생성: `npx wrangler queues create analytics-edge-retry`
4. `wrangler.toml`의 `ORIGIN_URL`을 Tunnel 주소로 교체
5. 비밀값 등록 (wrangler.toml·저장소에 쓰지 않는다)
   ```bash
   npx wrangler secret put EDGE_SHARED_SECRET   # ingest의 EDGE_SHARED_SECRET과 같은 값, 32자 이상
   ```
6. `npx wrangler deploy` (빌드 명령 `worker-build --release`가 자동 실행된다)
7. 라우트·커스텀 호스트명(Cloudflare for SaaS) 연결: `collect.<agency-domain>`, 광고주별 `track.<광고주 도메인>`

### 필요한 바인딩

| 이름 | 종류 | 용도 |
|---|---|---|
| `PROJECTS` | KV | `project:{public_key}` → `ProjectConfig` JSON (api가 설정 변경 시 기록, §4.3) |
| `RETRY_QUEUE` | Queue producer + consumer | 원점 장애 시 수집 배치 버퍼 (`analytics-edge-retry`) |
| `ORIGIN_URL` | var | ingest Tunnel 주소 |
| `EDGE_SHARED_SECRET` | secret | Worker↔ingest HMAC 키 |

## 배포 후에만 확인할 수 있는 것

- 압축 번들 1MB 이하, 요청당 CPU p99 5ms 이하 (D5). 현재 wasm-opt 전 gzip 약 0.8MB.
- `request.cf`의 `region`·`city` 값 형식 (현재 그대로 64자로 잘라 저장)
- 커스텀 호스트명에서 `Set-Cookie: Domain=<등록 가능 도메인>`이 실제로 저장되는지 (Safari ITP 포함)
- Queue 재시도 지연(`delaySeconds`)·`max_retries` 동작, `waitUntil` 안의 10초 타임아웃

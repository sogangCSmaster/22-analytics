# 22-analytics 아키텍처 설계서

> 상태: Draft v0.1 · 2026-11-20
> 기준 문서: 「중복 없이, 실제 매출로」 가이드, AMM-AD-31-001 (매체 간 광고 매출 중복 소거 운용 및 구현 매뉴얼)

---

## 0. 요약

**22-analytics는 여러 웹사이트(프로젝트)의 방문·이벤트·전자상거래 데이터를 수집하는 경량 웹 분석 플랫폼입니다.** 핵심 기능은 **자사몰 주문번호(`order_id`) 기준으로 매체 간 중복 계상을 소거하고, 여러 기여 모델로 매출을 배분하는 것**입니다.

| 항목 | 결정 |
|---|---|
| 서비스 형태 | **대행사 내부 전용 도구 (SI 납품)**. 대행사가 광고주별 프로젝트를 운영하며 외부 판매·과금은 하지 않음 |
| UI/UX | 토스 계열 PG 관리자 페이지 같은 **깔끔한 B2B 어드민 톤** (§12.1) |
| 수집 입구 | Cloudflare Worker (엣지): 스크립트 배포, 이벤트 수신, 고객사 도메인 TLS |
| 플랫폼 | AWS 서울 리전 (`ap-northeast-2`). 데이터는 국내에만 저장 |
| 이벤트 저장·분석 | ClickHouse, Graviton EC2 소형 인스턴스 1대로 시작 |
| 메타데이터 | PostgreSQL (초기에는 같은 EC2, 이후 RDS) |
| 원본 아카이브 | S3. 원본 데이터의 최종 기준(source of truth)이며 재처리에 사용 |
| 백엔드 | Rust (Axum, Tokio, `clickhouse` crate, `sqlx`) |
| 대시보드 | Rust + Dioxus (Web/WASM SPA). 차트는 ECharts JS interop |
| 트래커 | TypeScript로 작성한 순수 JS. gzip 기준 3KB 이하 목표 |
| 원점 연결 | Cloudflare Tunnel. EC2 인바운드 포트를 열지 않음 |

### 목표
1. 여러 조직과 프로젝트를 관리하고, 프로젝트별로 스크립트 스니펫과 키를 발급합니다.
2. 페이지뷰, 커스텀 이벤트, 전자상거래 이벤트를 수집합니다. 수집 항목은 프로젝트 설정으로 켜고 끕니다.
3. S2S 주문 API(확정·취소·부분환불)를 매출의 기준으로 삼습니다.
4. 중복 소거 리포트(매체 합산 / 소거 실매출 / 중복률)와 Last·First·Linear·Time-Decay 기여 모델을 제공합니다.
5. 월 1억 이벤트 규모를 **월 $100 안팎**의 인프라 비용으로 처리합니다.

### 비목표 (1차 범위 밖)
- 네이티브 앱 SDK와 Web-to-App 딥링크 (Phase 4)
- 뷰스루(노출 기반) 기여. 매뉴얼 FIM-05에 따라 클릭 기반 소거로 범위를 합의합니다.
- 매체 광고비 API 자동 연동. 초기에는 CSV 업로드로 ROAS를 계산합니다.
- 세션 리플레이, 히트맵

---

## 1. 전체 구조

```
 ┌──────────────── 고객 사이트 (www.client.com) ────────────────┐
 │  <script src="https://track.client.com/t.js" data-key=...>   │
 │   └─ 이벤트 배치 전송 (sendBeacon / fetch keepalive)          │
 │  주문 서버 ── S2S: POST /v1/orders (Bearer sk_...)           │
 └───────────────────────────┬──────────────────────────────────┘
                             │ HTTPS (고객사 CNAME 또는 공용 도메인)
 ┌───────────────────────────▼────── Cloudflare ────────────────┐
 │  Worker "edge"                                               │
 │   · GET  /t.js          스크립트 (엣지·브라우저 장기 캐시)     │
 │   · POST /v1/collect    키·Origin 검증, 봇 필터, geo 부착,    │
 │                         visitor 쿠키(Set-Cookie) 발급        │
 │   · POST /v1/orders     secret key 검증                      │
 │  KV: 프로젝트 설정 캐시 · Queue: 원점 장애 시 버퍼            │
 │  Cloudflare for SaaS: track.<고객사> 커스텀 호스트명 TLS      │
 │  Pages: 대시보드 정적 파일 (Dioxus WASM)                     │
 └───────────────────────────┬──────────────────────────────────┘
                             │ Cloudflare Tunnel (cloudflared, 아웃바운드 전용)
 ┌───────────────────────────▼────── AWS 서울 ──────────────────┐
 │  EC2 (Graviton, m7g.large 급)  ─ docker compose              │
 │   ├─ ingest   (Rust): 배치 수신 → ClickHouse insert          │
 │   │                   + 세그먼트 파일 → S3 아카이브           │
 │   ├─ api      (Rust/Axum): 대시보드·리포팅 API, 관리 API      │
 │   ├─ jobs     (Rust): 기여 연산, 정합성 검증, 롤업, 백업      │
 │   ├─ clickhouse                                              │
 │   └─ postgres                                                │
 │  EBS gp3 · S3 (raw 아카이브, CH cold tier, 백업)             │
 └──────────────────────────────────────────────────────────────┘
```

### 1.1 데이터 흐름

1. 트래커는 이벤트를 메모리 큐에 모았다가 다음 시점에 **배치로** 전송합니다.
   - 2초가 지나거나 10건이 쌓였을 때
   - 페이지가 숨겨질 때(`visibilitychange`, `pagehide`)
2. Worker는 받은 배치를 검증하고 보강한 뒤 Tunnel을 통해 `ingest`로 전달합니다(`ctx.waitUntil`). 브라우저에는 즉시 `204`를 응답합니다.
3. 원점으로 전달하지 못하면 Worker가 배치를 **Cloudflare Queue**에 넣고, Queue consumer가 재시도합니다.
4. `ingest`는 1초 또는 1만 행 단위로 ClickHouse에 insert합니다. 같은 배치를 로컬 세그먼트 파일(NDJSON)에도 append하고, 5분마다 zstd로 압축해 S3에 업로드합니다.
5. `jobs`는 주문 변경분을 감지해 기여 연산을 수행하고 결과를 `attributed_revenue`에 씁니다.
6. 대시보드는 `api`를 통해 ClickHouse의 롤업 테이블과 기여 결과를 조회합니다.

### 1.2 왜 이 구조인가

- **요청 단가**: 이벤트 수신은 요청 수가 압도적으로 많습니다. Workers 요청 단가가 API Gateway나 Lambda보다 훨씬 쌉니다. 클라이언트 배치로 요청 수를 다시 3~5배 줄입니다.
- **AWS 전송비 0**: AWS로 들어오는 트래픽은 무료입니다. 나가는 트래픽은 대시보드 응답뿐이라 작습니다.
- **ITP 대응(FIM-02)**: `track.client.com` CNAME과 Cloudflare for SaaS를 쓰면 서버가 **HttpOnly 1st-party 쿠키**를 발급할 수 있습니다. 이 쿠키는 Safari의 7일 제한을 받지 않습니다.
- **보안**: Tunnel만 쓰므로 EC2 보안그룹의 인바운드가 0입니다. NAT Gateway도 필요 없습니다(퍼블릭 서브넷 + 아웃바운드만 사용).
- **ClickHouse**: 고정비로 예측할 수 있고, 대시보드가 0.1초 미만으로 응답하며, 커스텀 이벤트 속성으로 자유롭게 필터링할 수 있습니다.

---

## 2. 멀티테넌시와 키

대행사 내부 도구이므로 테넌트 구조는 단순합니다.

- **운영 주체는 대행사 하나**입니다. 시스템 관리자는 대행사 직원입니다.
- **Organization은 "광고주"**를 뜻합니다. 광고주 하나가 사이트 여러 개(Project)를 가질 수 있습니다.
- 회원가입, 플랜, 과금 기능은 없습니다. 계정은 관리자가 초대해서 만듭니다.
- 광고주 담당자에게는 필요하면 **읽기 전용(viewer) 계정**을 발급해 리포트만 공유할 수 있습니다.

```
Organization (광고주)
 └─ Project (웹사이트 1개)  ── project_id (u32), public_key "pk_..."
     ├─ Secret Keys         ── "sk_..." (S2S 주문 API, 리포팅 API)
     ├─ Allowed Origins     ── 수집을 허용할 도메인 목록
     ├─ Custom Hostname     ── track.client.com (선택)
     ├─ Tracking Settings   ── 수집 항목 토글
     ├─ Channel Rules       ── 매체 정규화 규칙
     └─ Attribution Settings── lookback, 기본 모델, 반감기
```

- 전역 역할(`users.is_super_admin`)을 가진 대행사 관리자는 모든 광고주에 접근합니다.
- 그 외 사용자와 조직은 N:M(`memberships`)이고 역할은 `admin | analyst | viewer`입니다. 대행사 AE는 담당 광고주에만 배정하고, 광고주 담당자는 `viewer`로 둡니다.
- `pk_`는 공개 키입니다. 스니펫에 노출되므로 **Origin 검증과 rate limit**으로 보호합니다.
- `sk_`는 비밀 키입니다. 형식은 `sk_live_<prefix 8자>_<랜덤 40자>`이고, DB에는 prefix와 전체 키의 sha256만 저장합니다. 키 자체가 고엔트로피라 argon2 없이도 안전하고, 매 요청 검증이 빠릅니다. 생성 시 한 번만 보여줍니다.
- 프로젝트마다 `salt`(32바이트)를 둡니다. `member_id_hash`를 다시 해싱하는 데 씁니다(§9).

---

## 3. 스크립트 발급과 트래커

### 3.1 발급 스니펫

```html
<!-- 커스텀 호스트명 사용 시 (권장) -->
<script async src="https://track.client.com/t.js" data-key="pk_7Hq2..."></script>

<!-- 기본 (공용 도메인) -->
<script async src="https://collect.<agency-domain>/t.js" data-key="pk_7Hq2..."></script>

<!-- 로드 전 호출을 위한 큐 스텁 -->
<script>window.at=window.at||function(){(at.q=at.q||[]).push(arguments)};</script>
```

- `t.js`는 **모든 프로젝트가 공유하는 하나의 불변 파일**입니다.
  - 버전마다 `/t.v{N}.js`를 두고 `immutable` 캐시를 적용합니다.
  - `/t.js`는 최신 버전을 가리키며 `max-age=3600`입니다.
- 프로젝트 설정은 Worker가 `/v1/config?k=pk_...`로 내려줍니다. KV에서 읽고 `max-age=300`입니다. 트래커는 설정을 `localStorage`에 캐시해 다음 페이지에서 바로 씁니다.
- 수집 항목 토글은 **서버(Worker)에서도 다시 적용**합니다. 설정이 바뀐 직후 옛 설정으로 들어온 이벤트도 걸러지도록 하기 위해서입니다.

### 3.2 프로젝트 설정 (대시보드 토글)

| 설정 | 기본값 | 설명 |
|---|---|---|
| `auto_pageview` | on | 페이지 로드 시 `page_view` |
| `spa_mode` | on | `history.pushState`/`popstate`를 감지해 `page_view` |
| `outbound_links` | off | 외부 링크 클릭 |
| `file_downloads` | off | pdf, zip 등 다운로드 |
| `scroll_depth` | off | 25/50/75/90% |
| `ecommerce` | off | 전자상거래 이벤트 수집 허용 |
| `client_purchase_as_order` | off | S2S가 없는 프로젝트에서 클라이언트 `purchase`를 주문으로 인정 (§5.3) |
| `click_id_params` | `gclid, fbclid, abx_tid, click_id, n_keyword, NaPm, _a1_kad` | 터치포인트로 인정할 클릭 ID 파라미터 |
| `capture_query_params` | `[]` | URL에서 추가로 저장할 파라미터 |
| `strip_query_params` | `[]` | 개인정보가 섞일 수 있는 파라미터 제거 |
| `cookie_domain` | auto | `.client.com` |
| `respect_dnt` | off | Do Not Track 존중 |

### 3.3 JS API

```js
at('event', 'signup_complete', { plan: 'pro', method: 'kakao' });

// 전자상거래 (GA4 이벤트명과 호환)
at('ecommerce', 'view_item',     { items: [{ id: 'SKU1', name: '패딩', price: 89000, qty: 1 }] });
at('ecommerce', 'add_to_cart',   { items: [...], value: 89000, currency: 'KRW' });
at('ecommerce', 'begin_checkout',{ items: [...], value: 89000 });
at('ecommerce', 'purchase',      { order_id: 'ORD-9901', value: 100000, items: [...] });

at('identify', '<sha256(회원ID)>');   // 로그인 시 (FIM-03)
at('consent', false);                  // 수집 중단 + 쿠키 삭제
at('pageview');                        // 수동 페이지뷰
at('getVisitorId', id => ...);         // 비 HttpOnly 모드에서 주문서에 실어 보낼 때
```

### 3.4 방문자와 세션 식별

| 모드 | visitor_id 저장 | 수명 |
|---|---|---|
| **A. 커스텀 호스트명 (권장)** | Worker가 `Set-Cookie: _at_vid=...; Domain=.client.com; HttpOnly; Secure; SameSite=Lax; Max-Age=31536000` | ITP 제한 없음 |
| B. 공용 도메인 | 트래커가 JS로 `_at_vid` 쿠키 생성 | Safari에서 최대 7일 |

- **첫 방문 경쟁 조건**: 첫 요청이 동시에 여러 개 나가면 visitor_id가 여러 개 생길 수 있습니다. 이를 막기 위해 트래커가 후보 ID(`crypto.randomUUID()`)를 만들어 함께 보냅니다. Worker는 쿠키가 없을 때 이 후보를 채택합니다.
- **S2S에서 visitor_id 얻기**: 모드 A에서는 고객사 주문 서버가 같은 상위 도메인의 HttpOnly 쿠키 `_at_vid`를 **서버 측에서 직접 읽어** S2S 페이로드에 넣습니다. 매뉴얼 2.D의 흐름과 같습니다.
- **세션**: 트래커가 `_at_sid` 쿠키로 관리합니다. 30분 동안 활동이 없거나 캠페인 파라미터가 바뀌면 새 세션입니다.

### 3.5 전송 포맷 (`POST /v1/collect`)

```json
{
  "k": "pk_7Hq2...",
  "v": "a1b2...-uuid",          // visitor 후보 ID (모드 B에서는 확정 ID)
  "s": "c3d4...-uuid",          // session_id
  "sv": 3,                      // 트래커 버전
  "e": [
    {
      "id": "uuid",             // event_id (재전송 중복 방지)
      "n": "page_view",
      "t": 1763631912345,       // 클라이언트 ms
      "u": "https://www.client.com/p/123?utm_source=naver&n_keyword=패딩",
      "r": "https://search.naver.com/...",
      "ti": "상품 상세",
      "sw": 390, "sh": 844,
      "p": { "plan": "pro" },   // 커스텀 속성 (문자열/숫자/불리언)
      "ec": null                // 전자상거래 페이로드
    }
  ]
}
```

- 제한: 배치당 최대 50건, 64KB 이하. 속성 키는 최대 50개, 값은 최대 500자입니다.
- 응답은 `204`입니다. 모드 A에서 쿠키가 없으면 `Set-Cookie`를 붙입니다.

---

## 4. Edge Worker

**구현**: `workers-rs`(Rust → WASM)를 우선 검토합니다. 번들 크기나 콜드스타트가 문제가 되면 이 부분만 TypeScript로 바꿉니다. 로직이 얇아서 교체 비용이 낮습니다.

### 4.1 수집 요청 처리 순서

1. `pk` → 프로젝트 설정을 KV에서 조회합니다(Worker 메모리 캐시 60초). 없거나 비활성이면 `204` 후 폐기합니다.
2. `Origin`이나 `Referer`가 허용 도메인 목록에 있는지 확인합니다. 아니면 폐기합니다.
3. 봇을 필터링합니다.
   - Cloudflare `cf.botManagement`(사용 가능 시) 또는 UA 블랙리스트
   - headless 지표
   - 같은 IP에서 분당 N건 이상
4. 보강합니다.
   - `cf.country`, `cf.region`, `cf.city`
   - UA 파싱 결과(device, browser, os)
   - 서버 수신 시각 `received_at`
   - 클라이언트 시각 보정: 클라이언트와 서버 시각 차이가 ±24시간을 넘으면 서버 시각을 사용합니다.
5. **IP는 저장하지 않습니다.** rate limit과 geo 판단에만 쓰고 버립니다.
6. 수집 토글과 파라미터 strip을 적용합니다.
7. `ctx.waitUntil(fetch(ingest))`로 전달하고, 실패하면 `QUEUE.send(batch)`합니다.
8. 브라우저에는 즉시 `204`를 응답합니다.

### 4.2 원점 인증

- Worker에서 Tunnel로 가는 요청에는 `X-Edge-Auth: HMAC(secret, body)` 헤더를 붙입니다.
- `ingest`는 Tunnel 인터페이스로만 listen합니다(compose 내부 네트워크).

### 4.3 KV 동기화

`api`가 프로젝트 설정을 변경하면 Cloudflare API로 KV에 씁니다. 설정 버전 번호를 함께 기록합니다.

---

## 5. S2S 주문 API

### 5.1 엔드포인트

```
POST https://track.client.com/v1/orders      (또는 collect.<agency-domain>)
Authorization: Bearer sk_live_...
Idempotency-Key: <선택. 없으면 order_id + event_type + 금액으로 계산>
```

**주문 확정**

```json
{
  "event_type": "PURCHASE_CONFIRMED",
  "order_id": "ORD-20261120-009901",
  "visitor_id": "5c8a0d27-0861-4c76-b062-bc938f8c6833",
  "member_id_hash": "8d969eef...",
  "currency": "KRW",
  "gross_amount": 110000,
  "net_amount": 100000,
  "ordered_at": "2026-11-20T18:45:12+09:00",
  "items": [{ "id": "SKU1", "price": 100000, "qty": 1 }]
}
```

**취소·환불**

```json
{ "event_type": "ORDER_CANCELLED", "order_id": "ORD-20261120-009901",
  "refund_amount": 100000, "cancelled_at": "2026-11-21T09:12:04+09:00" }

{ "event_type": "ORDER_REFUNDED",  "order_id": "ORD-20261120-009901",
  "refund_amount": 30000,  "refunded_at": "..." }   // 부분 환불
```

- 응답: `202 {"accepted": true}`. 실패하면 `4xx`와 오류 코드를 반환합니다.
- 일괄 전송 엔드포인트 `POST /v1/orders:batch`는 최대 500건입니다. 백필과 재동기화용입니다.
- `net_amount`의 정의(쿠폰·포인트·배송비 처리)는 프로젝트별 설정 문서로 남깁니다. 매뉴얼 2.A(2)를 따릅니다.

### 5.2 처리 규칙

- 모든 주문 이벤트는 `order_events`에 **append-only**로 쌓습니다.
- 현재 상태는 `orders`(ReplacingMergeTree)에 투영합니다.
  - `effective_amount = net_amount − Σ refund_amount` (0 미만이면 0)
  - `status`: `CONFIRMED | PARTIALLY_REFUNDED | CANCELLED`
- 같은 `order_id`가 여러 번 들어와도 매출은 **한 번만** 확정됩니다. 매뉴얼 2.E(1)과 같습니다.
- 주문 상태가 바뀌면 `order_changes`에 변경을 기록하고, 이 기록이 기여 연산을 다시 트리거합니다(§7).

### 5.3 S2S가 없는 프로젝트

`client_purchase_as_order = on`이면 트래커의 `purchase` 이벤트(`order_id` 필수)를 `source='client'`인 주문으로 투영합니다. 같은 `order_id`의 S2S 주문이 들어오면 S2S가 우선합니다. 대시보드에는 **"클라이언트 기준: 유실 가능"** 배지를 표시합니다(FIM-04).

---

## 6. 데이터 모델

### 6.1 ClickHouse

```sql
-- 원본 이벤트
CREATE TABLE events
(
    project_id      UInt32,
    event_id        UUID,
    ts              DateTime64(3, 'UTC'),
    received_at     DateTime64(3, 'UTC'),
    event_name      LowCardinality(String),
    visitor_id      UUID,
    session_id      UUID,
    member_hash     String,                       -- 프로젝트 salt로 재해싱한 값, 없으면 ''
    -- 페이지
    url             String,
    host            LowCardinality(String),
    path            String,
    title           String,
    referrer        String,
    referrer_host   LowCardinality(String),
    -- 캠페인 (원본 그대로 저장)
    utm_source      String,
    utm_medium      String,
    utm_campaign    String,
    utm_content     String,
    utm_term        String,
    click_id        String,
    click_id_type   LowCardinality(String),       -- gclid / fbclid / abx_tid / ...
    channel         LowCardinality(String),       -- 수집 시점 정규화 결과 (§8)
    -- 환경
    country         LowCardinality(String),
    region          LowCardinality(String),
    city            LowCardinality(String),
    device_type     LowCardinality(String),
    browser         LowCardinality(String),
    os              LowCardinality(String),
    screen_w        UInt16,
    -- 커스텀 / 전자상거래
    `props.key`     Array(LowCardinality(String)),   -- 커스텀 속성 (Nested)
    `props.value`   Array(String),
    ec_order_id     String,
    ec_value        Int64,                        -- KRW 정수 (통화 최소단위)
    ec_currency     LowCardinality(String),
    `items.id`      Array(String),
    `items.name`    Array(String),
    `items.price`   Array(Int64),
    `items.qty`     Array(UInt32)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (project_id, toDate(ts, 'Asia/Seoul'), event_name, visitor_id, ts)
TTL toDateTime(ts) + INTERVAL 25 MONTH DELETE
SETTINGS index_granularity = 8192;
```

```sql
-- 터치포인트: 광고·캠페인 유입 (append-only, 매뉴얼 2.C)
CREATE TABLE touchpoints
(
    project_id   UInt32,
    visitor_id   UUID,
    member_hash  String,
    ts           DateTime64(3, 'UTC'),
    session_id   UUID,
    utm_source   String, utm_medium String, utm_campaign String,
    utm_content  String, utm_term String,
    click_id     String,
    click_id_type LowCardinality(String),
    referrer_host LowCardinality(String),
    landing_url  String
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (project_id, visitor_id, ts);

-- 세션 첫 이벤트 중 캠페인 파라미터, 클릭 ID, 또는 외부 referrer가 있는 것만
CREATE MATERIALIZED VIEW mv_touchpoints TO touchpoints AS
SELECT project_id, visitor_id, member_hash, ts, session_id,
       utm_source, utm_medium, utm_campaign, utm_content, utm_term,
       click_id, click_id_type, referrer_host, url AS landing_url
FROM events
WHERE event_name = 'page_view'
  AND (click_id != '' OR utm_source != '' OR props['_ext_ref'] = '1');
```

> 실제 스키마는 `migrations/clickhouse/0001_init.sql`이 기준입니다. 이벤트의 `channel`과 `utm_*`는 **세션 범위**입니다: 트래커가 세션 랜딩 URL·referrer(`l`)를 함께 보내고, 세션 안의 모든 이벤트가 랜딩 기준 채널을 갖습니다(GA의 세션 소스와 같음). 터치포인트 MV는 `is_session_start = 1 AND channel NOT IN ('direct', '(internal)')` 조건을 씁니다.

> 터치포인트는 "세션을 시작시킨 유입"만 기록합니다. Direct 유입은 터치포인트로 만들지 않습니다. GA의 last non-direct와 같은 방식입니다.

```sql
-- 주문 이벤트 원본 (append-only)
CREATE TABLE order_events
(
    project_id     UInt32,
    order_id       String,
    event_type     LowCardinality(String),   -- PURCHASE_CONFIRMED / ORDER_CANCELLED / ORDER_REFUNDED
    source         LowCardinality(String),   -- s2s / client
    idempotency_key String,
    visitor_id     Nullable(UUID),
    member_hash    String,
    currency       LowCardinality(String),
    gross_amount   Int64,
    net_amount     Int64,
    refund_amount  Int64,
    occurred_at    DateTime64(3, 'UTC'),
    received_at    DateTime64(3, 'UTC'),
    raw            String                    -- 원본 JSON (감사용)
)
ENGINE = ReplacingMergeTree(received_at)
ORDER BY (project_id, order_id, idempotency_key);

-- 주문 현재 상태 (ingest가 주문 이벤트마다 재계산해 insert)
CREATE TABLE orders
(
    project_id       UInt32,
    order_id         String,
    source           LowCardinality(String),
    status           LowCardinality(String),
    visitor_id       Nullable(UUID),
    member_hash      String,
    currency         LowCardinality(String),
    net_amount       Int64,
    refunded_amount  Int64,
    effective_amount Int64,
    ordered_at       DateTime64(3, 'UTC'),
    version          UInt64                  -- 단조 증가 (ms)
)
ENGINE = ReplacingMergeTree(version)
PARTITION BY toYYYYMM(ordered_at)
ORDER BY (project_id, order_id);

-- 크로스 디바이스 연결 (FIM-03)
CREATE TABLE identity_links
(
    project_id  UInt32,
    member_hash String,
    visitor_id  UUID,
    first_seen  DateTime64(3, 'UTC'),
    last_seen   DateTime64(3, 'UTC')
)
ENGINE = ReplacingMergeTree(last_seen)
ORDER BY (project_id, member_hash, visitor_id);

-- 기여 결과
CREATE TABLE attributed_revenue
(
    project_id    UInt32,
    model         LowCardinality(String),   -- last_click / first_click / linear / time_decay / raw_sum
    lookback_days UInt8,
    order_id      String,
    touch_index   UInt16,                   -- 0 = 미귀속(Unattributed)
    channel       LowCardinality(String),
    utm_campaign  String,
    touch_ts      DateTime64(3, 'UTC'),
    ordered_at    DateTime64(3, 'UTC'),
    amount        Int64,
    is_assist     UInt8,                    -- last_click에서 0원이 된 접점
    computed_at   DateTime64(3, 'UTC'),
    is_deleted    UInt8
)
ENGINE = ReplacingMergeTree(computed_at, is_deleted)
PARTITION BY toYYYYMM(ordered_at)
ORDER BY (project_id, model, lookback_days, order_id, touch_index);
```

### 6.2 롤업 (대시보드용)

용도별로 좁은 롤업을 둡니다 (`migrations/clickhouse/0004_rollups.sql`). 처음의 넓은 `daily_stats`
(키: 채널·호스트·경로·기기·국가)는 원본과 크기가 비슷해 0004에서 이 셋으로 바꿨습니다.

```sql
-- 개요·추이·채널
CREATE TABLE daily_channel
(
    project_id UInt32, day Date, channel LowCardinality(String),
    pageviews SimpleAggregateFunction(sum, UInt64),
    events    SimpleAggregateFunction(sum, UInt64),
    entrances SimpleAggregateFunction(sum, UInt64),
    visitors  AggregateFunction(uniqCombined(14), UUID),
    sessions  AggregateFunction(uniqCombined(14), UUID)
)
ENGINE = AggregatingMergeTree PARTITION BY toYYYYMM(day)
ORDER BY (project_id, day, channel);

-- 페이지 (페이지뷰·세션 진입 이벤트만, visitors는 페이지뷰 방문자)
CREATE TABLE daily_pages
(
    project_id UInt32, day Date, host LowCardinality(String), path String CODEC(ZSTD(1)),
    pageviews SimpleAggregateFunction(sum, UInt64),
    entrances SimpleAggregateFunction(sum, UInt64),
    visitors  AggregateFunction(uniqCombined(14), UUID)
)
ENGINE = AggregatingMergeTree PARTITION BY toYYYYMM(day)
ORDER BY (project_id, day, host, path);

-- 이벤트 이름별
CREATE TABLE daily_events
(
    project_id UInt32, day Date, event_name LowCardinality(String),
    events   SimpleAggregateFunction(sum, UInt64),
    visitors AggregateFunction(uniqCombined(14), UUID)
)
ENGINE = AggregatingMergeTree PARTITION BY toYYYYMM(day)
ORDER BY (project_id, day, event_name);

-- mv_daily_channel / mv_daily_pages / mv_daily_events: events → 각 롤업 (toDate(ts, 프로젝트 타임존))
```

- 개요·추이·채널은 `daily_channel`, 페이지는 `daily_pages`, 이벤트 이름별 집계는 `daily_events`를 조회합니다.
  기기·국가 화면이 생기면 같은 방식으로 좁은 롤업을 추가합니다. 오늘 데이터만 `events`를 직접 조회합니다.
- 순 방문자·세션은 롤업과 원본 조회 모두 `uniqCombined(14)`를 씁니다.
- 커스텀 이벤트 탐색(속성 필터)은 `events`를 직접 조회합니다. 정렬키가 `(project_id, date, event_name)`이라 스캔 범위가 작습니다.
- 실시간 화면(최근 30분)도 `events`를 직접 조회합니다.
- 프로젝트 타임존은 기본 `Asia/Seoul`입니다. 다른 타임존 프로젝트는 롤업 MV에서 프로젝트 테이블을 dictionary로 조회해 처리합니다.

### 6.3 PostgreSQL (메타데이터)

```
users(id, email, password_hash, name, is_super_admin, created_at, last_login_at)
organizations(id, name, memo, created_at)      -- 광고주
memberships(user_id, org_id, role)
projects(id serial→u32, org_id, name, public_key, timezone, currency,
         salt bytea, status, settings jsonb, settings_version, created_at)
project_origins(project_id, origin)
api_keys(id, project_id, prefix, key_hash, scopes, created_by, last_used_at, revoked_at)
custom_hostnames(project_id, hostname, cf_hostname_id, status, verified_at)
channel_rules(id, project_id NULL=전역 기본값, priority, match jsonb, channel, label)
attribution_settings(project_id, lookback_days[], default_model, half_life_hours,
                     include_organic bool, include_referral bool)
ad_spend(project_id, day, channel, campaign, amount)   -- CSV 업로드 (ROAS)
sessions(id, user_id, expires_at)                      -- 대시보드 로그인
audit_log(id, org_id, user_id, action, target, payload jsonb, at)
job_watermarks(job, project_id, watermark)
```

---

## 7. 중복 소거와 기여 엔진

매뉴얼 2.E를 구현합니다. 로직은 `crates/core`에 **순수 함수**로 두어 단위 테스트를 쉽게 하고, 실행은 `jobs`가 담당합니다.

### 7.1 실행 주기

| 잡 | 주기 | 대상 |
|---|---|---|
| `attribution-incremental` | 5분 | 직전 watermark 이후 `orders.version`이 바뀐 주문 |
| `attribution-full` | 매일 04:00 KST | 최근 35일 주문 전체. 늦게 도착한 터치포인트, identity 변경, 채널 규칙 변경 반영 |
| `attribution-rebuild` | 수동 | 채널 규칙이나 lookback 변경 시 기간 지정 |
| `conservation-check` | 매시간 | 매출 보존 법칙 검증 (§7.4) |

### 7.2 알고리즘 (주문 1건)

```
입력: order(order_id, visitor_id?, member_hash, effective_amount, ordered_at)
      settings(lookback_days ∈ {7, 30}, half_life, channel_rules)

1. 식별자 확장
   ids = {visitor_id}
       ∪ identity_links[member_hash] 의 visitor_id 들
       (member_hash가 있으면 같은 회원의 다른 기기도 포함)

2. 터치포인트 조회
   T = touchpoints WHERE visitor_id ∈ ids
                     AND ordered_at − lookback ≤ ts ≤ ordered_at
       ORDER BY ts ASC
   각 t에 channel = normalize(t, channel_rules) 적용 (§8, 현재 규칙 기준)

3. T가 비어 있으면 → touch_index=0, channel='(unattributed)', amount=R

4. 모델별 배분 (R = effective_amount, KRW 정수)
   last_click : T[-1] ← R, 나머지 0 (is_assist=1)
   first_click: T[0]  ← R
   linear     : 각 floor(R / n), 나머지 원 단위는 마지막 접점에
   time_decay : w_i = 2^(−(ordered_at − ts_i) / half_life)   (기본 반감기 7일)
                a_i = floor(R · w_i / Σw), 나머지는 마지막 접점에
   raw_sum    : T의 "서로 다른 channel"마다 R 전액
                → 각 매체가 자기 성과로 주장하는 매출을 근사한 값

5. CANCELLED(effective_amount=0)이면 기존 행을 is_deleted=1로 갱신
```

- **정수 배분과 나머지 처리**로 Σ가 항상 R과 정확히 같도록 보장합니다.
- `raw_sum`은 매체 대시보드 수치를 근사한 값입니다. 실제 매체 리포트 수치는 Phase 4의 광고 API 연동으로 대체할 수 있습니다.

### 7.3 리포트 지표 (매뉴얼 1.D)

| 지표 | 정의 |
|---|---|
| 매체 합산 매출 | Σ `raw_sum` |
| 중복 소거 실매출 | Σ `effective_amount` (= 어느 모델이든 Σ amount) |
| 중복률 | 1 − 실매출 / 합산 매출 |
| 매체별 귀속 매출 | 선택한 모델·lookback 기준 channel별 Σ amount |
| 어시스트 수 | last_click 기준 `is_assist=1` 접점 수 |
| ROAS | 귀속 매출 / `ad_spend` |
| 중복 매체 조합 | 주문별 channel 집합의 빈도 (예: naver_sa + meta_da) |

### 7.4 매출 보존 검증

```sql
SELECT model, lookback_days, count() AS broken
FROM (
  SELECT model, lookback_days, order_id, sum(amount) AS s
  FROM attributed_revenue FINAL
  WHERE project_id = {p} AND model != 'raw_sum' AND is_deleted = 0
  GROUP BY model, lookback_days, order_id
) a
JOIN (SELECT order_id, effective_amount FROM orders FINAL WHERE project_id = {p}) o
USING order_id
WHERE s != o.effective_amount
GROUP BY model, lookback_days;
```

불일치가 1건이라도 있으면 알림을 보내고 해당 주문을 재연산합니다. 매뉴얼에 따르면 불일치는 "연산 로직 결함"입니다.

---

## 8. 채널 정규화

매뉴얼 1.D(1)의 UTM 네이밍 문제를 시스템에서 흡수합니다.

- 원본 값(`utm_*`, `click_id_type`, `referrer_host`)은 **그대로 저장**합니다.
- `channel`은 규칙으로 계산합니다.
  - 수집 시점 값은 `events.channel`에 저장하고 트래픽 리포트에 씁니다.
  - 기여 연산은 **현재 규칙으로 다시 계산**합니다. 규칙을 고치면 과거 기여 결과에도 소급 적용됩니다.
- 규칙은 우선순위 순으로 평가하며, 처음 일치하는 규칙을 적용합니다. 비교는 소문자와 trim 기준입니다.

| 우선순위 | 조건 | channel |
|---|---|---|
| 프로젝트 규칙 | 예: `utm_source in (naver, Naver, NAVER) AND utm_medium = cpc` | `naver_sa` |
| 기본 | `click_id_type = gclid` | `google_ads` |
| 기본 | `click_id_type = fbclid` 또는 `utm_source in (facebook, instagram, meta)` | `meta` |
| 기본 | `n_keyword`, `NaPm` 존재 또는 `utm_source = naver` | `naver` |
| 기본 | `abx_tid` 존재 | `utm_source` 기반으로 판정, 없으면 `dfinery` |
| 기본 | 검색엔진 referrer (naver, google, daum, bing) | `organic_search` |
| 기본 | 그 외 외부 referrer | `referral` |
| 기본 | 그 외 | `(other)` |

대시보드에는 정규화되지 않은 원본 값 조합 상위 목록을 보여주는 **"미분류 소스" 화면**을 두고, 여기서 바로 규칙을 추가할 수 있게 합니다.

---

## 9. 개인정보와 보안

- **저장 위치**: 모든 영속 데이터는 AWS 서울 리전에 둡니다. Cloudflare는 전송 경로이며 Queue에만 일시 보관합니다. 개인정보처리방침의 처리위탁·국외이전 항목은 법무 검토를 받습니다.
- **IP 미저장**: geo는 Cloudflare 헤더로만 판정합니다.
- **회원 ID**:
  - 고객사가 `sha256(회원ID)`를 보내면 서버가 `sha256(project_salt ‖ hash)`로 **재해싱**해 저장합니다.
  - 따라서 프로젝트끼리 같은 회원을 연결할 수 없습니다.
  - 평문으로 보이는 값(이메일 형식이나 전화번호 패턴)은 거부합니다.
- **URL 정리**: `email`, `phone`, `token` 같은 민감 파라미터를 기본 strip 목록으로 제거합니다. 프로젝트별로 목록을 추가할 수 있습니다.
- **동의**: `at('consent', false)`를 호출하면 즉시 수집을 중단하고 쿠키를 삭제합니다. 프로젝트 설정으로 "동의 전 수집 안 함" 모드를 지원합니다.
- **보존기간**: 기본 25개월이며 플랜별 TTL로 조정합니다. 프로젝트를 삭제하면 `ALTER TABLE ... DELETE WHERE project_id`를 비동기로 실행합니다.
- **키 관리**:
  - `sk_`는 해시만 저장합니다.
  - Worker와 원점 사이 HMAC 비밀값, ClickHouse·Postgres 비밀번호는 AWS SSM Parameter Store(무료 표준 티어)에 둡니다.
- **대시보드 인증**:
  - 이메일·비밀번호(argon2id)와 HttpOnly 세션 쿠키를 씁니다.
  - Google OAuth는 Phase 3입니다.
  - 테넌트 격리는 모든 API에서 `project_id ∈ 사용자 접근 가능 프로젝트`를 강제합니다. ClickHouse 쿼리 빌더가 `project_id` 조건을 필수 인자로 받게 해서 누락을 컴파일 단계에서 막습니다.
- **Rate limit**:
  - Worker: `pk`별, IP별 분당 상한
  - S2S: `sk`별 초당 상한

---

## 10. 인프라 (AWS 서울)

### 10.1 초기 구성 (Phase 1~2)

| 리소스 | 사양 | 비고 |
|---|---|---|
| EC2 | `m7g.large` (2 vCPU, 8GB), Graviton | docker compose: ingest, api, jobs, clickhouse, postgres, cloudflared |
| EBS | gp3 100GB (3000 IOPS 기본) | ClickHouse hot 데이터와 Postgres |
| S3 | `22a-raw` (원본 세그먼트), `22a-ch-cold` (CH tiered), `22a-backup` | Lifecycle: raw는 90일 후 Glacier IR |
| VPC | 퍼블릭 서브넷 1개, **NAT 없음**, 인바운드 0 | S3 Gateway Endpoint (무료) |
| SSM | Session Manager로 접속 (SSH 포트 없음) | Parameter Store로 비밀값 관리 |
| IaC | Terraform | `infra/terraform` |

- **ClickHouse 메모리 설정**: 8GB 장비에서는 `max_server_memory_usage_to_ram_ratio=0.6`, `async_insert=0`(ingest가 직접 배치), `max_threads=2`로 둡니다.
- **Tiered storage**: 30일이 지난 파티션은 S3 disk로 `MOVE`합니다(`TTL ... TO VOLUME 'cold'`).
- **백업**:
  - ClickHouse: `clickhouse-backup`으로 매일 S3에 증분 백업
  - Postgres: `pg_dump`로 매일 S3에 백업
  - 원본 S3 세그먼트가 있으므로 최악의 경우에도 재적재로 복구할 수 있습니다.
- **모니터링**:
  - CloudWatch 기본 지표와 디스크·메모리 알람(최소 구성)
  - 애플리케이션 지표(수집 지연, Queue 적체, 보존 검증 실패)는 `api`의 `/metrics` 엔드포인트와 알림 웹훅(Slack)으로 처리합니다.
  - CloudWatch Logs는 7일 보존으로 설정합니다.

### 10.2 확장 경로

| 신호 | 조치 |
|---|---|
| 월 5억 이벤트 이상, 또는 CPU 70% 이상 지속 | EC2를 `m7g.xlarge`나 `2xlarge`로 수직 확장 |
| ClickHouse와 API의 리소스 경합 | API와 jobs를 `t4g.small`로 분리하고, Postgres를 RDS `db.t4g.micro`로 이전 |
| 가용성 요구 | ClickHouse 2노드 + Keeper 3노드(`ReplicatedMergeTree`) |
| 수십억 행/일 | 샤딩, 또는 S3 원본을 유지한 채 ClickHouse Cloud로 이전 검토 |

---

## 11. 비용 추정 (월 1억 이벤트, 대략치. 착수 시 재확인)

| 항목 | 월 비용 |
|---|---|
| Cloudflare Workers Paid (기본 $5 + 배치로 약 3천만 요청) | $10~15 |
| Cloudflare Queue (장애 시에만 사용) | ~$0 |
| Cloudflare for SaaS (무료 할당 내) | $0 |
| EC2 m7g.large (1년 Savings Plan 기준) | $45~55 |
| EBS gp3 100GB | ~$10 |
| S3 (원본 zstd 압축 약 10GB/월 누적, 백업) | $1~5 |
| 기타 (CloudWatch, SSM, 데이터 아웃) | ~$5 |
| **합계** | **약 $75~95** |

이벤트당 저장 용량은 ClickHouse 압축 후 약 50~100바이트로 가정합니다. 이 기준이면 연간 약 1.2억 행당 10GB 수준입니다.

---

## 12. 리포지토리 구조

```
22-analytics/
├─ Cargo.toml                 # workspace
├─ crates/
│  ├─ core/                   # 공유 타입, 이벤트 스키마, 채널 정규화, 기여 모델 (순수 로직)
│  ├─ ch/                     # ClickHouse 클라이언트 래퍼, 마이그레이션, 행 타입 (ProjectScope로 project_id 강제)
│  ├─ pg/                     # Postgres 메타데이터 접근, 마이그레이션, 개발 시드
│  ├─ ingest/                 # Axum: /internal/collect, /internal/orders → CH + S3 세그먼트
│  ├─ api/                    # Axum: 대시보드 API, 관리 API, 리포팅 API
│  ├─ jobs/                   # 기여 연산, 보존 검증, 백업 트리거, KV 동기화
│  └─ web/                    # Dioxus (WASM) 대시보드
├─ edge/
│  └─ worker/                 # Cloudflare Worker (workers-rs), wrangler.toml
├─ tracker/                   # TypeScript → esbuild, 크기 예산 CI 체크 (≤3KB gz)
├─ migrations/
│  ├─ clickhouse/
│  └─ postgres/               # sqlx migrate
├─ infra/
│  ├─ terraform/              # VPC, EC2, S3, IAM, SSM
│  └─ compose/                # docker-compose.yml, clickhouse config, cloudflared
└─ docs/
   ├─ ARCHITECTURE.md
   └─ (이후) API.md, TRACKER.md, RUNBOOK.md
```

### 대시보드 (Dioxus)

- 정적 WASM SPA로 빌드해 **Cloudflare Pages(무료)**에 배포합니다. API는 `api.analytics.<agency-domain>`에서 Tunnel을 거쳐 `api`로 연결됩니다.
- 공유 타입: `crates/core`의 요청·응답 DTO를 `web`과 `api`가 함께 씁니다.
- 차트: ECharts를 `wasm-bindgen`이나 `document::eval`로 감싼 `Chart` 컴포넌트 하나를 둡니다.
- 1차 화면:
  1. 프로젝트 목록과 생성
  2. 설치(스니펫, 설치 검증)
  3. 개요(방문자, 세션, 페이지뷰, 추이)
  4. 실시간
  5. 채널·캠페인
  6. 페이지
  7. 이벤트(속성 필터)
  8. 전자상거래(퍼널: view_item → add_to_cart → begin_checkout → purchase)
  9. **중복 소거 리포트**(합산 / 소거 / 중복률, 모델·lookback 전환, 매체 조합)
  10. 설정(수집 토글, 도메인, 키, 채널 규칙, 기여 설정, 커스텀 호스트명)

### 12.1 UI/UX 가이드: PG 관리자 페이지 톤

목표는 토스 계열 결제(PG) 관리자 페이지처럼 **숫자가 주인공인 깔끔한 B2B 어드민**입니다. 토스의 로고, 서체(Toss Product Sans), 고유 일러스트 같은 브랜드 자산은 쓰지 않습니다. 톤과 원칙만 참고하고, **자체 디자인 토큰**을 정의해 `crates/web`에 컴포넌트 라이브러리로 구현합니다.

**디자인 원칙**
1. **숫자 우선**: 핵심 지표는 큰 숫자(32~40px, Bold)로 보여주고, 라벨과 증감은 작은 회색 글씨로 붙입니다. 금액은 `1,234,000원`처럼 원 단위와 천 단위 구분을 씁니다.
2. **넉넉한 여백과 낮은 대비의 면**: 흰 배경 위에 연회색 카드(`radius 16~20px`, 그림자 없음 또는 아주 약하게)를 둡니다.
3. **강조색 하나**: 파란색 계열 primary 하나만 쓰고, 빨강(감소·오류)과 초록(증가·정상)은 상태 표시에만 씁니다.
4. **쉬운 말**: "Attribution" 대신 "매출 기여", "Deduplicated revenue" 대신 "실제 매출"처럼 마케터 용어로 씁니다. 매뉴얼의 "쉽게 말해" 톤을 따릅니다.
5. **빈 상태와 설치 안내**: 데이터가 없을 때는 다음 행동(스니펫 복사, 설치 확인)을 바로 할 수 있게 합니다.

**디자인 토큰 (초안)**

| 토큰 | 값 | 용도 |
|---|---|---|
| `font-family` | Pretendard Variable (OFL), 숫자는 `tabular-nums` | 전체 |
| `color-primary` | `#3182F6` 계열 | 주요 버튼, 선택 상태, 차트 1번 계열 |
| `color-text` / `-sub` / `-weak` | `#191F28` / `#4E5968` / `#8B95A1` | 본문 / 보조 / 비활성 |
| `color-bg` / `-card` / `-line` | `#FFFFFF` / `#F2F4F6` / `#E5E8EB` | 배경 / 카드 / 구분선 |
| `color-up` / `color-down` | 초록 / 빨강 계열 | 증감 표시 |
| `radius` | 8 / 12 / 16 / 20 | 입력 / 버튼 / 카드 / 큰 카드 |
| `space` | 4px 그리드 (4, 8, 12, 16, 24, 32, 48) | 간격 |
| `type-scale` | 13 / 15 / 17 / 20 / 26 / 32 / 40 | 캡션 ~ 대형 지표 |

**레이아웃**
- 왼쪽 고정 사이드바: 상단에 광고주·프로젝트 전환기, 그 아래 메뉴(개요, 실시간, 유입, 페이지, 이벤트, 전자상거래, **실제 매출**, 설정)
- 상단 바: 기간 선택(오늘, 7일, 30일, 직접 선택), 비교 기간 토글, 사용자 메뉴
- 본문: 상단에 지표 카드 4개 → 추이 차트 → 상세 표
- 표: 줄무늬 없이 얇은 구분선, 행 hover, 오른쪽 정렬 숫자, 정렬·CSV 내보내기
- 상세 보기는 오른쪽 **사이드 패널(드로어)**로 열어 목록 맥락을 유지합니다. 예: 주문 하나를 클릭하면 그 주문의 터치포인트 타임라인과 모델별 배분을 보여줍니다.

**핵심 화면: 실제 매출(중복 소거)**
- 상단 카드 3개: `매체 합산 매출`(취소선 회색) → `실제 매출`(primary, 대형) → `중복률`
- 모델 선택: 세그먼트 컨트롤(Last-Click, First-Click, Linear, Time-Decay)과 lookback(7일, 30일)
- 매체별 표: 귀속 매출, 어시스트, 광고비, ROAS
- 매체 조합 차트: 중복이 집중되는 조합 (예: 네이버 → 메타)
- 안내 문구 카드: 매뉴얼 1.D(3)의 광고주 설명 문안을 그대로 복사할 수 있게 제공합니다.

**컴포넌트 목록 (Dioxus)**
`Button`, `TextField`, `Select`, `SegmentedControl`, `Toggle`, `DateRangePicker`, `StatCard`, `Card`, `Table`(정렬, 페이지네이션), `Drawer`, `Modal`, `Toast`, `Badge`, `Tabs`, `EmptyState`, `Skeleton`, `CodeBlock`(복사 버튼), `Chart`(ECharts 래퍼, 토큰 색상 적용)

라이트 모드를 기본으로 하고, 다크 모드는 토큰만 정의해 두고 Phase 3에서 적용합니다.

---

## 13. 로드맵

| 단계 | 범위 | 완료 기준 |
|---|---|---|
| **Phase 0** 기반 | workspace 골격, Terraform, compose, CH·PG 마이그레이션, CI(fmt, clippy, test, tracker 크기), **디자인 토큰과 기본 컴포넌트** | `docker compose up`으로 로컬 전체 기동 |
| **Phase 1** 수집 MVP | 트래커(pageview, SPA, custom event, 배치), Worker(collect, config, t.js), ingest, S3 아카이브, 조직·프로젝트·키 관리, 개요·실시간·채널·페이지 화면, 설치 검증 | 실제 사이트 1곳에서 7일간 유실률 1% 미만, 대시보드 p95 300ms 미만 |
| **Phase 2** 매출·중복 소거 | 전자상거래 이벤트, S2S 주문 API(확정·취소·부분환불), 채널 규칙, 기여 엔진(Last/First/Linear/Time-Decay, 7·30일), 보존 검증, 중복 소거 리포트, 광고비 CSV와 ROAS | 매뉴얼 1.A 시나리오(ORD-9901) E2E 테스트 통과, 보존 검증 0건 |
| **Phase 3** 정확도·운영 | 커스텀 호스트명(HttpOnly 쿠키), identity graph, 미분류 소스 화면, Google OAuth, 알림, CSV/API 내보내기, 리포팅 API | Safari 사용자의 7일 이후 재방문 매칭 확인 |
| **Phase 4** 확장 | MMP(디파이너리, 에어브릿지) postback 수신, Web-to-App, 매체 광고비·리포트 API 연동, 광고주 공유용 리포트 링크·PDF | — |

---

## 14. 결정 사항 (2026-11-20 확정)

| # | 항목 | 결정 |
|---|---|---|
| D1 | 계정·운영 주체 | **AWS와 Cloudflare 모두 대행사 명의 계정**에 구축합니다. **개발은 로컬에서 먼저 완료**하고(§16), 개발이 끝난 뒤 대행사 계정과 도메인을 받아 Terraform으로 한 번에 구성합니다. 납품물에 `RUNBOOK.md`(배포, 백업 복구, 장애 대응, 키 교체), Terraform 상태(S3 backend + DynamoDB lock, 대행사 계정)와 운영자 계정 목록을 포함합니다. 납품 후 유지보수는 별도 계약 범위로 둡니다. |
| D2 | 도메인 | **대행사 소유 도메인의 서브도메인**을 씁니다. 대행사 도메인의 DNS는 Cloudflare로 옮깁니다. <br>· `collect.<agency-domain>`: 공용 수집과 `t.js` <br>· `analytics.<agency-domain>`: 대시보드(Pages) <br>· `api.analytics.<agency-domain>`: 대시보드 API(Tunnel) <br>· 광고주별 `track.<광고주 도메인>`: 커스텀 호스트명(권장, 광고주가 CNAME으로 `collect.<agency-domain>`을 가리킴) |
| D3 | `net_amount` 기본 정의 | **상품 판매가 합계 − 쿠폰·즉시할인 − 포인트·적립금 사용액, 배송비 제외, 부가세 포함**으로 정합니다. 매체 픽셀과 비교할 수 있도록 `gross_amount`(할인 전 결제 총액)도 함께 받아 저장합니다. 광고주 재무팀 기준이 다르면 프로젝트 설정의 메모와 연동 가이드로 개별 합의합니다(매뉴얼 2.A). |
| D4 | 기여 대상 유입 | **자연검색과 추천(referral)을 기여 대상에 포함**합니다(`include_organic=true`, `include_referral=true`). Direct는 제외합니다(last non-direct). 이 유입을 빼면 Last-Click이 광고 매체를 과대평가하기 때문입니다. 리포트에는 "광고 매체만 보기" 필터를 제공합니다. |
| D5 | Worker 언어 | **workers-rs(Rust)**로 시작합니다. Phase 1에서 압축 번들 1MB 이하, 요청당 CPU p99 5ms 이하를 확인하고, 기준을 넘으면 Worker만 TypeScript로 교체합니다. 교체해도 프로토콜과 스키마는 그대로입니다. |

## 15. 남은 확인 사항 (대행사 측)

1. **대행사 도메인 이름**과 Cloudflare DNS 이전 가능 여부
2. **AWS·Cloudflare 계정 접근 권한**: 개발용 IAM 역할(최소 권한)과 Cloudflare API 토큰(Workers, KV, Queues, Pages, Tunnel, Custom Hostnames 범위)
3. **개인정보 법무 검토**: 광고주 개인정보처리방침의 처리위탁(대행사) 항목과 Cloudflare 경유 고지 문안
4. **파일럿 광고주 1곳**: Phase 1 검증(7일간 유실률 측정)에 쓸 실제 사이트

## 16. 로컬 개발

배포 없이 로컬에서 전체 흐름을 돌릴 수 있습니다. Cloudflare Worker 역할은 ingest의 **엣지 에뮬레이션 모드**(`EDGE_EMULATION=true`)가 대신합니다. 검증·보강 로직은 Worker와 같은 `analytics-core::collect`를 씁니다.

```bash
make up        # ClickHouse + Postgres (docker compose)
make migrate   # 스키마 적용 (두 번 실행해도 안전)
make seed      # 개발용 광고주·프로젝트 (pk_devlocal0000000000000000)
make ingest    # t.js 빌드 + 수집 서버 → http://localhost:8081/dev/ 테스트 페이지
make web       # 대시보드 → dx serve
make test      # 전체 테스트
```

| 운영 | 로컬 |
|---|---|
| Cloudflare Worker `/t.js`, `/v1/config`, `/v1/collect` | ingest 같은 경로 (에뮬레이션) |
| Worker → Tunnel → ingest `/internal/collect` (HMAC) | 같은 프로세스 안에서 바로 파이프라인으로 |
| KV 프로젝트 설정 | Postgres 조회 + 60초 메모리 캐시 |
| S3 원본 아카이브 | `ARCHIVE_DIR` 로컬 디렉터리 (`.ndjson.zst`) |
| 거부 시 204 (사유 숨김) | 204 + `x-at-status` 헤더에 사유 (디버깅용) |

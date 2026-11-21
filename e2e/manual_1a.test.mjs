// 매뉴얼 1.A 시나리오 E2E (docs/MVP_PLAN.md 완료 기준 3)
//
//   네이버 검색광고 클릭 → 30분 뒤 메타 광고 클릭 → 5분 뒤 100,000원 결제(주문 서버 S2S)
//
// 실제 프로세스(ingest 엣지 에뮬레이션·api·jobs)와 브라우저(트래커 t.js)로 돌리고,
// 대시보드가 보는 API 리포트에서 다음을 확인한다.
//   매체 합산 200,000 / 실제 매출 100,000 / 중복률 50%
//   Last-Click 메타 100,000 (네이버는 어시스트) / Linear 50,000 + 50,000
//   중복 경로 naver_sa → meta, 주문 상세 터치 2개
// 사전 조건: make up && make migrate (DB 기동·스키마)
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { ApiClient, INGEST, UA, build, clickhouse, jobs, kstDate, playwright, startStack, waitFor } from "./lib.mjs";

const MIN = 60_000;

/** 스텁(`at.q` 큐)이 실제 트래커로 바뀔 때까지 기다린다 */
const trackerReady = (page) =>
  page.waitForFunction(() => typeof window.at === "function" && !String(window.at).includes("at.q"));
let stop = () => {};
const api = new ApiClient();
const ctx = {};

before(async () => {
  build();
  jobs(["migrate"]);
  jobs(["create-admin", "--email", "e2e-admin@example.com", "--password", "e2e-password-123", "--reset-password"]);
  stop = await startStack();
  await api.call("POST", "/v1/auth/login", { email: "e2e-admin@example.com", password: "e2e-password-123" });
});

after(() => stop());

test("광고주·프로젝트·허용 도메인·비밀 키를 API로 만든다", async () => {
  const run = Date.now().toString(36);
  const org = await api.call("POST", "/v1/orgs", { name: `E2E 광고주 ${run}` });
  const project = await api.call("POST", `/v1/orgs/${org.id}/projects`, { name: `E2E 쇼핑몰 ${run}` });
  await api.call("POST", `/v1/projects/${project.id}/origins`, { origin: "localhost" });
  const key = await api.call("POST", `/v1/projects/${project.id}/api-keys`, {});
  assert.match(key.secret, /^sk_live_/);
  Object.assign(ctx, { projectId: project.id, publicKey: project.public_key, secret: key.secret });
});

test("브라우저: 네이버 검색광고 → 메타 광고 순서로 두 번 방문한다", async () => {
  // 주문 시각을 지금보다 2시간 전으로 두고, 클릭은 35분·5분 전 (매뉴얼의 18:10 / 18:40 / 18:45 간격)
  const order = Math.floor((Date.now() - 2 * 3600 * 1000) / MIN) * MIN;
  ctx.orderedAt = order;
  const { chromium } = playwright();
  const browser = await chromium.launch({ args: [`--user-agent=${UA}`] });
  const page = await (await browser.newContext()).newPage();
  const base = `${INGEST}/dev/e2e.html?k=${ctx.publicKey}`;

  await page.clock.install({ time: order - 35 * MIN });
  await page.goto(`${base}&utm_source=naver&utm_medium=cpc&utm_campaign=winter&n_keyword=패딩`);
  await trackerReady(page);
  await page.clock.setSystemTime(order - 5 * MIN);
  await page.goto(`${base}&utm_source=facebook&utm_medium=paid&fbclid=IwAR_e2e`);
  await trackerReady(page);
  await page.goto("about:blank"); // pagehide → sendBeacon으로 남은 이벤트 전송

  // pagehide에 보낸 sendBeacon 응답(Set-Cookie)은 비동기로 도착하므로 기다린다
  let vid;
  await waitFor(async () => {
    vid = (await page.context().cookies(INGEST)).find((c) => c.name === "_at_vid");
    return !!vid;
  }, "방문자 쿠키", 10000).catch(() => {});
  assert.ok(vid, "서버가 HttpOnly 방문자 쿠키를 발급해야 한다");
  assert.equal(vid.httpOnly, true);
  ctx.visitorId = vid.value;
  await browser.close();

  // 두 세션의 유입이 터치포인트로 적재될 때까지 대기
  await waitFor(
    async () =>
      (await clickhouse(`SELECT count() FROM touchpoints WHERE project_id = ${ctx.projectId}`)) === "2",
    "터치포인트 2건",
  );
  const channels = await clickhouse(
    `SELECT arrayStringConcat(groupArray(channel), ',') FROM (SELECT channel FROM touchpoints WHERE project_id = ${ctx.projectId} ORDER BY ts)`,
  );
  assert.equal(channels, "naver_sa,meta");
});

test("주문 서버가 S2S로 결제 확정을 보내고, 재전송해도 한 번만 센다", async () => {
  const body = {
    event_type: "PURCHASE_CONFIRMED",
    order_id: `ORD-9901-${ctx.projectId}`,
    visitor_id: ctx.visitorId,
    currency: "KRW",
    net_amount: 100000,
    ordered_at: new Date(ctx.orderedAt).toISOString(),
  };
  ctx.orderId = body.order_id;
  for (let i = 0; i < 2; i++) {
    const res = await fetch(`${INGEST}/v1/orders`, {
      method: "POST",
      headers: { authorization: `Bearer ${ctx.secret}`, "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    assert.equal(res.status, 202, await res.text());
  }
  const n = await clickhouse(`SELECT count() FROM orders FINAL WHERE project_id = ${ctx.projectId}`);
  assert.equal(n, "1");
});

test("기여 연산 잡을 돌리면 매출 보존 법칙이 지켜진다", () => {
  jobs(["attribution", "--project", String(ctx.projectId)]);
  // 재실행해도 결과가 같아야 한다 (멱등)
  jobs(["attribution", "--project", String(ctx.projectId), "--full"]);
  const out = jobs(["conservation-check", "--project", String(ctx.projectId)]);
  assert.match(out, /통과/);
});

test("실제 매출 리포트: 합산 200,000 / 실제 100,000 / 중복률 50%", async () => {
  const day = kstDate(ctx.orderedAt);
  const q = `from=${day}&to=${day}&lookback=7`;
  const last = await api.call("GET", `/v1/projects/${ctx.projectId}/reports/revenue?${q}&model=last_click`);
  assert.equal(last.raw_sum, 200000);
  assert.equal(last.deduplicated, 100000);
  assert.equal(last.duplicate_amount, 100000);
  assert.equal(last.duplicate_rate, 0.5);
  assert.equal(last.orders, 1);
  const byChannel = Object.fromEntries(last.channels.map((c) => [c.channel, c]));
  assert.equal(byChannel.meta.attributed, 100000);
  assert.equal(byChannel.naver_sa.attributed, 0);
  assert.equal(byChannel.naver_sa.assists, 1);
  assert.equal(byChannel.naver_sa.raw_claimed, 100000);

  const linear = await api.call("GET", `/v1/projects/${ctx.projectId}/reports/revenue?${q}&model=linear`);
  const lin = Object.fromEntries(linear.channels.map((c) => [c.channel, c.attributed]));
  assert.deepEqual(lin, { naver_sa: 50000, meta: 50000 });

  const first = await api.call("GET", `/v1/projects/${ctx.projectId}/reports/revenue?${q}&model=first_click`);
  const fst = Object.fromEntries(first.channels.map((c) => [c.channel, c.attributed]));
  assert.deepEqual(fst, { naver_sa: 100000, meta: 0 });
});

test("중복 경로와 주문 상세가 대시보드 API로 보인다", async () => {
  const day = kstDate(ctx.orderedAt);
  const overlap = await api.call("GET", `/v1/projects/${ctx.projectId}/reports/overlap?from=${day}&to=${day}&lookback=7`);
  assert.equal(overlap.multi_channel_orders, 1);
  assert.deepEqual(overlap.paths[0].channels, ["naver_sa", "meta"]);

  const detail = await api.call(
    "GET",
    `/v1/projects/${ctx.projectId}/reports/orders/${encodeURIComponent(ctx.orderId)}?lookback=7`,
  );
  assert.equal(detail.order.effective_amount, 100000);
  assert.deepEqual(
    detail.touches.map((t) => t.channel),
    ["naver_sa", "meta"],
  );
});

test("취소가 들어오면 실제 매출에서 빠진다", async () => {
  const res = await fetch(`${INGEST}/v1/orders`, {
    method: "POST",
    headers: { authorization: `Bearer ${ctx.secret}`, "content-type": "application/json" },
    body: JSON.stringify({
      event_type: "ORDER_CANCELLED",
      order_id: ctx.orderId,
      refund_amount: 100000,
      cancelled_at: new Date(ctx.orderedAt + 30 * MIN).toISOString(),
    }),
  });
  assert.equal(res.status, 202, await res.text());
  jobs(["attribution", "--project", String(ctx.projectId)]);
  const day = kstDate(ctx.orderedAt);
  const r = await api.call("GET", `/v1/projects/${ctx.projectId}/reports/revenue?from=${day}&to=${day}&lookback=7&model=last_click`);
  assert.equal(r.deduplicated, 0);
  assert.equal(r.orders, 0);
});

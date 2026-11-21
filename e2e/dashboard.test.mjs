// 대시보드 E2E: 실제 dx 빌드 + API + 브라우저.
// 대시보드 리뷰 지적사항을 회귀 테스트로 고정한다.
//  - P0: 설정 화면에서 프로젝트를 전환한 뒤 저장해도 이전 프로젝트 설정이 새 프로젝트로 넘어가지 않는다
//  - P1: 목록 행을 키보드(Enter)로 열고, 드로어를 Esc로 닫는다
//  - 화면을 돌아다니는 동안 콘솔 오류·페이지 오류가 없다
// 사전 조건: make up && make migrate
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { ApiClient, DASHBOARD, INGEST, UA, build, buildDashboard, jobs, playwright, serveStatic, startStack, waitFor } from "./lib.mjs";

const api = new ApiClient();
const ctx = {};
let stopStack = () => {};
let stopWeb = () => {};
let browser;
let page;
const errors = [];

before(async () => {
  build();
  const dir = buildDashboard();
  jobs(["migrate"]);
  jobs(["create-admin", "--email", "e2e-admin@example.com", "--password", "e2e-password-123", "--reset-password"]);
  stopStack = await startStack();
  stopWeb = await serveStatic(dir, 18080);
  await api.call("POST", "/v1/auth/login", { email: "e2e-admin@example.com", password: "e2e-password-123" });

  // 광고주 하나에 프로젝트 둘: A는 전자상거래·스크롤 켬, B는 둘 다 끔
  const run = Date.now().toString(36);
  const org = await api.call("POST", "/v1/orgs", { name: `대시보드 E2E ${run}` });
  const a = await api.call("POST", `/v1/orgs/${org.id}/projects`, { name: `A몰 ${run}` });
  const b = await api.call("POST", `/v1/orgs/${org.id}/projects`, { name: `B몰 ${run}` });
  for (const [p, on] of [[a, true], [b, false]]) {
    const cur = await api.call("GET", `/v1/projects/${p.id}/settings`);
    await api.call("PUT", `/v1/projects/${p.id}/settings`, { ...cur.settings, ecommerce: on, scroll_depth: on });
  }
  Object.assign(ctx, { org, a, b, run });

  // 실시간: A에만 방금 방문 3건
  await api.call("POST", `/v1/projects/${a.id}/origins`, { origin: "localhost" });
  for (let v = 0; v < 3; v++) {
    const res = await fetch(`${INGEST}/v1/collect`, {
      method: "POST",
      headers: { origin: "http://localhost", "user-agent": UA, "content-type": "text/plain" },
      body: JSON.stringify({ k: a.public_key, v: crypto.randomUUID(), s: crypto.randomUUID(), sv: 1,
        e: [{ id: crypto.randomUUID(), n: "page_view", t: Date.now(), u: "http://localhost/p", ss: true }] }),
    });
    if (res.status !== 204) throw new Error(`collect ${res.status}`);
  }
  // 주문 12건 (최근 주문 목록 2쪽)
  const key = await api.call("POST", `/v1/projects/${a.id}/api-keys`, {});
  const orders = Array.from({ length: 12 }, (_, i) => ({
    event_type: "PURCHASE_CONFIRMED", order_id: `P1-${run}-${i}`, currency: "KRW", net_amount: 10000 + i,
    ordered_at: new Date(Date.now() - (i + 1) * 60_000).toISOString(),
  }));
  const ores = await fetch(`${INGEST}/v1/orders:batch`, {
    method: "POST", headers: { authorization: `Bearer ${key.secret}`, "content-type": "application/json" },
    body: JSON.stringify({ orders }),
  });
  if (ores.status !== 202) throw new Error(`orders ${ores.status}`);
  jobs(["attribution", "--project", String(a.id)]);

  // 사용자: X(org 관리자), Z(org 관리자), Y(다른 광고주 소속)
  const other = await api.call("POST", "/v1/orgs", { name: `다른 광고주 ${run}` });
  for (const [who, orgId] of [["x", org.id], ["z", org.id], ["y", other.id]]) {
    const email = `${who}-${run}@example.com`;
    await api.call("POST", "/v1/users", { email, password: "member-password-1", name: who.toUpperCase() });
    await api.call("POST", `/v1/orgs/${orgId}/members`, { email, role: "admin" });
    ctx[who] = email;
  }

  const { chromium } = playwright();
  browser = await chromium.launch({ args: [`--user-agent=${UA}`] });
  page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error" && !/WebSocket|_dioxus|favicon/.test(m.text())) errors.push(m.text());
  });
});

after(async () => {
  await browser?.close();
  stopWeb();
  stopStack();
});

/** 사이드바 프로젝트 전환기로 같은 화면의 다른 프로젝트로 이동 */
async function switchProject(p, name, id) {
  await p.click(".project-switch");
  await p.click(`.pop-item:has-text("${name}")`);
  await p.waitForURL(new RegExp(`/p/${id}(/|$|\\?)`));
}

const sw = (title) => page.locator(`[role=switch][aria-label="${title}"]`);

test("로그인 화면에서 로그인하면 내 프로젝트로 들어간다", async () => {
  await page.goto(`${DASHBOARD}/login`);
  await page.fill("input[type=email]", "e2e-admin@example.com");
  await page.fill("input[type=password]", "e2e-password-123");
  await page.click('button:has-text("로그인")');
  await page.waitForURL(/\/p\/\d+/);
});

test("P0: 설정 화면에서 프로젝트를 바꾸고 저장해도 이전 프로젝트 설정이 넘어가지 않는다", async () => {
  const { a, b } = ctx;
  await page.goto(`${DASHBOARD}/p/${a.id}/settings?tab=tracking`);
  await sw("전자상거래 이벤트").waitFor();
  assert.equal(await sw("전자상거래 이벤트").getAttribute("aria-checked"), "true");

  // 같은 설정 탭에 머문 채 프로젝트 전환기로 B로 이동
  await page.click(".project-switch");
  await page.click(`.pop-item:has-text("${b.name}")`);
  await page.waitForURL(new RegExp(`/p/${b.id}/settings`));
  await page.waitForFunction(
    () => document.querySelector('[role=switch][aria-label="전자상거래 이벤트"]')?.getAttribute("aria-checked") === "false",
    null,
    { timeout: 10000 },
  );
  assert.equal(await sw("스크롤 깊이").getAttribute("aria-checked"), "false", "B의 설정이 보여야 한다");

  // B에서 스크롤만 켜고 저장
  await sw("스크롤 깊이").click();
  await page.click('button:has-text("저장")');
  await page.waitForFunction(() => !document.body.innerText.includes("저장 중"), null, { timeout: 10000 });
  await page.waitForTimeout(500);

  const sa = (await api.call("GET", `/v1/projects/${a.id}/settings`)).settings;
  const sb = (await api.call("GET", `/v1/projects/${b.id}/settings`)).settings;
  assert.equal(sb.scroll_depth, true, "B에 방금 켠 값이 저장돼야 한다");
  assert.equal(sb.ecommerce, false, "A의 전자상거래 설정이 B로 넘어오면 안 된다");
  assert.equal(sa.ecommerce, true, "A는 그대로");
  assert.equal(sa.scroll_depth, true, "A는 그대로");
});

test("P1: 광고주 목록 행을 Enter로 열고, 드로어를 Esc로 닫는다", async () => {
  await page.goto(`${DASHBOARD}/admin`);
  const row = page.locator(`tr:has-text("${ctx.org.name}")`).first();
  await row.waitFor();
  await row.focus();
  await page.keyboard.press("Enter");
  const dialog = page.locator("[role=dialog]");
  await dialog.waitFor();
  assert.equal(await dialog.getAttribute("aria-modal"), "true");
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "detached" });
});

test("주요 화면을 돌아도 콘솔 오류가 없다", async () => {
  for (const r of ["", "realtime", "acquisition", "pages", "events", "funnel", "revenue", "settings"]) {
    await page.goto(`${DASHBOARD}/p/${ctx.a.id}/${r}`);
    await page.waitForTimeout(1200);
  }
  assert.deepEqual(errors, []);
});

test("P1: 실시간 화면에서 프로젝트를 바꾸면 이전 프로젝트 숫자가 남지 않는다", async () => {
  await page.goto(`${DASHBOARD}/p/${ctx.a.id}/realtime`);
  const big = page.locator(".stat-value.xl").first();
  await waitFor(async () => (await big.innerText()) === "3", "A 실시간 3명", 15000);
  await switchProject(page, ctx.b.name, ctx.b.id);
  // 전환 직후부터 A의 숫자(3)가 보이면 안 된다
  for (let i = 0; i < 10; i++) {
    const shown = (await big.count()) ? await big.innerText() : "";
    assert.notEqual(shown, "3", "B 화면에 A의 실시간 숫자가 남아 있음");
    await page.waitForTimeout(100);
  }
  await waitFor(async () => (await big.innerText()) === "0", "B 실시간 0명", 15000);
});

test("P1: 최근 주문 2쪽에서 기간을 바꾸면 1쪽으로 돌아간다", async () => {
  await page.goto(`${DASHBOARD}/p/${ctx.a.id}/revenue`);
  const pager = page.locator(".pager");
  await pager.waitFor({ timeout: 15000 });
  assert.match(await pager.innerText(), /1–10\s*\/\s*12/);
  await page.click('.pager button:has-text("다음")');
  await waitFor(async () => /11–12/.test(await pager.innerText()), "2쪽", 10000);
  await page.click('[role=radio]:has-text("30일")');
  await waitFor(async () => /1–10/.test(await pager.innerText()), "기간 변경 후 1쪽", 10000);
});

test("P1: 주문 드로어를 연 채 다른 프로젝트로 가면 드로어가 닫힌다", async () => {
  await page.goto(`${DASHBOARD}/p/${ctx.b.id}/revenue`);
  await page.waitForTimeout(800);
  await switchProject(page, ctx.a.name, ctx.a.id);
  const row = page.locator("tr.clickable").first();
  await row.waitFor({ timeout: 15000 });
  await row.click();
  await page.locator("[role=dialog]").waitFor();
  await page.goBack(); // 브라우저 뒤로 가기 → B 실제 매출
  await page.waitForURL(new RegExp(`/p/${ctx.b.id}/revenue`));
  await page.locator("[role=dialog]").waitFor({ state: "detached", timeout: 5000 });
});

test("P1: 광고주 관리자가 다른 광고주 소속 이메일을 추가하면 이유를 알려준다", async () => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
  await p.goto(`${DASHBOARD}/login`);
  await p.fill("input[type=email]", ctx.x);
  await p.fill("input[type=password]", "member-password-1");
  await p.click('button:has-text("로그인")');
  await p.waitForURL(/\/p\/\d+|\/$/);
  await p.goto(`${DASHBOARD}/admin`);
  const row = p.locator(`tr:has-text("${ctx.org.name}")`).first();
  await row.waitFor();
  await row.focus();
  await p.keyboard.press("Enter");
  const dialog = p.locator("[role=dialog]");
  await dialog.waitFor();
  await dialog.locator('input[placeholder="name@company.com"]').fill(ctx.y);
  await dialog.locator('button:has-text("추가")').last().click();
  const alert = dialog.locator("[role=alert]");
  await alert.waitFor({ timeout: 10000 });
  assert.match(await alert.innerText(), /이 이메일은 추가할 수 없어요/);
  ctx.xPage = p;
});

test("P1: 관리자가 스스로 '보기'로 낮추면 패널이 닫히고 권한이 바로 반영된다", async () => {
  const p = ctx.xPage;
  const dialog = p.locator("[role=dialog]");
  await dialog.locator('input[placeholder="name@company.com"]').fill(ctx.x);
  await dialog.locator('[role=radio]:has-text("보기")').click();
  await dialog.locator('button:has-text("추가")').last().click();
  await dialog.waitFor({ state: "detached", timeout: 10000 });
  const row = p.locator(`tr:has-text("${ctx.org.name}")`).first();
  await waitFor(async () => (await row.innerText()).includes("보기"), "목록의 내 역할 = 보기", 10000);
  // 다시 열면 관리자 전용 '멤버 추가'가 없어야 한다
  await row.focus();
  await p.keyboard.press("Enter");
  await dialog.waitFor();
  assert.equal(await dialog.locator('input[placeholder="name@company.com"]').count(), 0);
  await p.close();
});

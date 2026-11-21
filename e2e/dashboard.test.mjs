// 대시보드 E2E: 실제 dx 빌드 + API + 브라우저.
// 대시보드 리뷰 지적사항을 회귀 테스트로 고정한다.
//  - P0: 설정 화면에서 프로젝트를 전환한 뒤 저장해도 이전 프로젝트 설정이 새 프로젝트로 넘어가지 않는다
//  - P1: 목록 행을 키보드(Enter)로 열고, 드로어를 Esc로 닫는다
//  - 화면을 돌아다니는 동안 콘솔 오류·페이지 오류가 없다
// 사전 조건: make up && make migrate
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { ApiClient, DASHBOARD, UA, build, buildDashboard, jobs, playwright, serveStatic, startStack } from "./lib.mjs";

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
  Object.assign(ctx, { org, a, b });

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

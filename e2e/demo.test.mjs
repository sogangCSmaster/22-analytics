// 데모 모드 E2E (백엔드 없음): GitHub Pages와 같은 `/22-analytics/` 경로 + 404.html SPA 폴백으로 띄운다.
//  - 로그인 화면으로 보내지고, 아무 이메일·비밀번호로 들어간다
//  - 모든 화면에 예시 데이터가 보이고, 콘솔·페이지 오류가 없다
//  - 깊은 주소를 새로 고쳐도 열린다 (404.html 폴백)
// 실행: cd e2e && npm run test:demo   (DEMO_DIR=<빌드 결과>로 빌드를 건너뛸 수 있다)
// 스크린샷: docs/screenshots/demo-*.png
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { ROOT, playwright } from "./lib.mjs";

const BASE = "/22-analytics";
const PORT = 18090;
const SITE = `http://localhost:${PORT}${BASE}`;
const SHOTS = join(ROOT, "docs/screenshots");
const TARGET = process.env.CARGO_TARGET_DIR || join(ROOT, "target");

let browser;
let page;
let stop = () => {};
const errors = [];

function buildDemo() {
  if (process.env.DEMO_DIR) return process.env.DEMO_DIR;
  execFileSync(
    "dx",
    ["build", "--platform", "web", "--package", "analytics-web", "--features", "demo", "--base-path", "22-analytics"],
    { cwd: ROOT, stdio: ["ignore", "ignore", "inherit"] },
  );
  return join(TARGET, "dx/analytics-web/debug/web/public");
}

/** GitHub Pages 흉내: `/22-analytics/` 아래 정적 파일, 없는 경로는 404.html을 404 상태로 */
async function servePages(dir) {
  copyFileSync(join(dir, "index.html"), join(dir, "404.html"));
  const http = await import("node:http");
  const { readFile, stat } = await import("node:fs/promises");
  const { extname } = await import("node:path");
  const types = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm", ".css": "text/css", ".png": "image/png", ".svg": "image/svg+xml" };
  const server = http.createServer(async (req, res) => {
    const path = decodeURIComponent(new URL(req.url, "http://x").pathname);
    if (!path.startsWith(BASE)) {
      res.writeHead(404).end("not found");
      return;
    }
    let rel = path.slice(BASE.length) || "/";
    if (rel.endsWith("/")) rel += "index.html";
    const file = join(dir, rel);
    try {
      if (rel.includes("..") || !(await stat(file)).isFile()) throw new Error();
      res.writeHead(200, { "content-type": types[extname(file)] || "application/octet-stream" });
      res.end(await readFile(file));
    } catch {
      res.writeHead(404, { "content-type": "text/html" });
      res.end(await readFile(join(dir, "404.html")));
    }
  });
  await new Promise((r) => server.listen(PORT, "127.0.0.1", r));
  return () => server.close();
}

before(async () => {
  stop = await servePages(buildDemo());
  mkdirSync(SHOTS, { recursive: true });
  const { chromium } = playwright();
  browser = await chromium.launch();
  page = await (await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: "ko-KR", timezoneId: "Asia/Seoul" })).newPage();
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    // 외부 웹폰트(CDN)가 막힌 환경의 리소스 오류는 앱 오류가 아니다
    if (m.type() === "error" && !/WebSocket|_dioxus|favicon|Failed to load resource/.test(m.text())) errors.push(m.text());
  });
  // 같은 사이트 자원(wasm·js·css·이미지)은 하나도 실패하면 안 된다 (문서 자체의 404 폴백은 제외)
  page.on("response", (r) => {
    const u = new URL(r.url());
    if (u.port === String(PORT) && r.status() >= 400 && r.request().resourceType() !== "document") errors.push(`${r.status()} ${u.pathname}`);
  });
});

after(async () => {
  await browser?.close();
  stop();
});

const settle = (ms = 900) => page.waitForTimeout(ms);

/** 첫 지표 카드 숫자 */
async function firstStat() {
  const v = page.locator(".stat-value").first();
  await v.waitFor({ timeout: 15000 });
  await page.waitForFunction(() => {
    const t = document.querySelector(".stat-value")?.textContent || "";
    return /[1-9]/.test(t);
  }, null, { timeout: 15000 });
  return v.innerText();
}

test("주소로 들어오면 로그인 화면으로 가고, 아무 값으로 로그인된다", async () => {
  await page.goto(`${SITE}/`);
  await page.waitForURL(new RegExp(`${BASE}/login`));
  await page.locator(".demo-hint").waitFor();
  assert.match(await page.locator(".demo-hint").innerText(), /아무 이메일·비밀번호/);
  // 데모 계정이 미리 채워져 있다
  assert.equal(await page.inputValue("input[type=email]"), "demo@binarycube.co.kr");
  await settle(400);
  await page.screenshot({ path: join(SHOTS, "demo-login.png") });
  // 아무 값 (이메일 형식이 아니어도)
  await page.fill("input[type=email]", `tester-${Math.random().toString(36).slice(2)}`);
  await page.fill("input[type=password]", Math.random().toString(36));
  await page.click('button:has-text("로그인")');
  await page.waitForURL(new RegExp(`${BASE}/p/1(/|$)`));
  const v = await firstStat();
  assert.match(v, /[1-9]/, `개요 숫자: ${v}`);
  assert.match(await page.locator(".demo-badge").innerText(), /데모 · 예시 데이터/);
  await page.locator(".chart, svg").first().waitFor();
  await settle(1200);
  await page.screenshot({ path: join(SHOTS, "demo-overview.png") });
});

test("모든 리포트 화면에 예시 데이터가 보인다", async () => {
  // 사이드바 링크로 이동 (라우터가 기본 경로를 붙이는지 확인)
  for (const [label, path] of [["실시간", "realtime"], ["유입", "acquisition"], ["페이지", "pages"], ["이벤트", "events"], ["전자상거래", "funnel"]]) {
    await page.click(`.nav a:has-text("${label}")`);
    await page.waitForURL(`${SITE}/p/1/${path}`);
    await page.locator(".demo-badge").waitFor();
    await settle();
    const text = await page.locator("main").innerText();
    assert.match(text, /[1-9]/, `${label}: 숫자가 없음`);
    assert.doesNotMatch(text, /불러오지 못|오류/, `${label}: 오류 표시`);
  }
  // 실시간: 활성 방문자 > 0
  await page.click('.nav a:has-text("실시간")');
  await page.waitForFunction(() => /[1-9]/.test(document.querySelector(".stat-value.xl")?.textContent || ""), null, { timeout: 15000 });
  // 이벤트: 행을 열면 속성 드로어
  await page.click('.nav a:has-text("이벤트")');
  const row = page.locator("tr.clickable").first();
  await row.waitFor({ timeout: 15000 });
  await row.click();
  const dialog = page.locator("[role=dialog]");
  await dialog.waitFor();
  await page.waitForFunction(() => /합계/.test(document.querySelector("[role=dialog]")?.textContent || ""), null, { timeout: 15000 });
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "detached" });
});

test("실제 매출: 주문 드로어를 열고 기여 모델을 바꾼다", async () => {
  await page.click('.nav a:has-text("실제 매출")');
  await page.waitForURL(`${SITE}/p/1/revenue`);
  await firstStat();
  const order = page.locator("tr.clickable").first();
  await order.waitFor({ timeout: 15000 });
  await settle(1200);
  await page.screenshot({ path: join(SHOTS, "demo-revenue.png") });
  // 모델 전환: 숫자가 바뀐다
  const table = page.locator("main .table").first();
  const before = await table.innerText();
  await page.click('[role=radio]:has-text("첫 클릭")');
  await page.waitForFunction((b) => document.querySelector("main .table")?.innerText !== b, before, { timeout: 15000 });
  await order.click();
  const dialog = page.locator("[role=dialog]");
  await dialog.waitFor();
  await page.waitForFunction(() => /ORD-/.test(document.querySelector("[role=dialog]")?.textContent || ""), null, { timeout: 15000 });
  await settle(600);
  await page.screenshot({ path: join(SHOTS, "demo-revenue-order.png") });
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "detached" });
});

test("설정 탭과 관리 화면이 열린다", async () => {
  for (const tab of ["install", "tracking", "origins", "rules", "attribution", "spend"]) {
    await page.goto(`${SITE}/p/1/settings?tab=${tab}`);
    await page.locator(".demo-badge").waitFor({ timeout: 15000 });
    await settle(700);
    assert.doesNotMatch(await page.locator("main").innerText(), /불러오지 못/, tab);
  }
  // 수집 설정 변경 → 저장 (데모 서버 상태에 남는다)
  await page.goto(`${SITE}/p/1/settings?tab=tracking`);
  const sw = page.locator('[role=switch][aria-label="파일 다운로드"]');
  await sw.waitFor({ timeout: 15000 });
  const was = await sw.getAttribute("aria-checked");
  await sw.click();
  await page.click('button:has-text("저장")');
  await settle(800);
  await page.reload();
  await sw.waitFor({ timeout: 15000 });
  assert.notEqual(await sw.getAttribute("aria-checked"), was, "새로 고친 뒤에도 바뀐 값");

  await page.click('.nav a:has-text("광고주·사용자")');
  await page.waitForURL(`${SITE}/admin`);
  await page.locator('tr:has-text("바이너리큐브 데모")').first().waitFor({ timeout: 15000 });
});

test("깊은 주소를 새로 고쳐도 열린다 (404.html 폴백, 로그인 유지)", async () => {
  await page.goto(`${SITE}/p/2/revenue`);
  await page.waitForURL(`${SITE}/p/2/revenue`);
  await firstStat();
  await page.reload();
  await firstStat();
  assert.match(page.url(), /\/22-analytics\/p\/2\/revenue$/);
});

test("로그아웃하면 로그인 화면으로", async () => {
  await page.goto(`${SITE}/p/1/`);
  await firstStat();
  await page.click('.user-block button[aria-label="로그아웃"]');
  await page.waitForURL(new RegExp(`${BASE}/login`), { timeout: 10000 });
  await page.goto(`${SITE}/p/1/pages`);
  await page.waitForURL(new RegExp(`${BASE}/login\\?next=`), { timeout: 15000 });
  assert.match(decodeURIComponent(page.url()), /next=\/p\/1\/pages/);
});

test("콘솔·페이지 오류가 없다", () => {
  assert.deepEqual(errors, []);
});

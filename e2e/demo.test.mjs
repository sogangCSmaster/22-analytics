// 데모 모드 E2E (백엔드 없음): GitHub Pages와 같은 `/22-analytics/` 경로 + 404.html SPA 폴백으로 띄운다.
//  - 로그인 화면으로 보내지고, 아무 이메일·비밀번호로 들어간다
//  - 모든 화면에 예시 데이터가 보이고, 콘솔·페이지 오류가 없다
//  - 깊은 주소를 새로 고쳐도 열린다 (404.html 폴백)
// 실행: cd e2e && npm run test:demo   (DEMO_DIR=<빌드 결과>로 빌드를 건너뛸 수 있다)
//  - 이전 기간 비교 토글이 카드·표를 바꾸고, 데모 안내는 한 번 뜨고 사라지는 알림이다
//  - 모바일(iPhone 13, 360·768px): 문서 가로 넘침·세그먼트 줄바꿈 없음, 상단 바 높이
// 스크린샷: docs/screenshots/demo-*.png (demo-compare-*, demo-mobile-*)
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
  // 데모 안내는 상단 바 배지가 아니라, 로그인 뒤 한 번 뜨고 사라지는 알림이다
  assert.equal(await page.locator(".demo-badge").count(), 0, "상단 바에 상시 데모 배지가 없어야 한다");
  const toast = page.locator(".toast");
  await toast.waitFor({ timeout: 5000 });
  assert.match(await toast.innerText(), /데모 화면입니다.*예시 데이터/);
  assert.equal(await toast.getAttribute("aria-live"), "polite");
  await toast.waitFor({ state: "detached", timeout: 8000 });
  // 같은 브라우저 세션에서는 다시 뜨지 않는다
  await page.reload();
  await firstStat();
  await settle(1200);
  assert.equal(await page.locator(".toast").count(), 0, "새로 고친 뒤에는 데모 알림이 다시 뜨지 않는다");
  // 개발용 '컴포넌트' 메뉴가 없다
  assert.equal(await page.locator('.nav a:has-text("컴포넌트")').count(), 0);
  assert.doesNotMatch(await page.locator(".nav").innerText(), /개발/);
  await page.locator(".chart, svg").first().waitFor();
  await settle(1200);
  await page.screenshot({ path: join(SHOTS, "demo-overview.png") });
});

/** 비교 표시 흔적: 증감 배지·지표 카드 증감·'이전 기간' 열 */
const compareMarks = () =>
  page.evaluate(() => ({
    chg: document.querySelectorAll("main .chg, main .delta").length,
    prevCols: [...document.querySelectorAll("main th")].filter((t) => /이전 기간|이전 전환율|변화/.test(t.textContent)).length,
  }));

async function setCompare(on) {
  const sw = page.locator(".compare [role=switch]");
  if ((await sw.getAttribute("aria-checked")) !== String(on)) await sw.click();
  assert.equal(await sw.getAttribute("aria-checked"), String(on));
}

test("이전 기간 비교: 켜면 카드·표에 이전 값과 변화가 보이고, 끄면 사라진다", async () => {
  for (const [path, needCols] of [["", true], ["acquisition", true], ["pages", true], ["events", true], ["funnel", true], ["revenue", true]]) {
    await page.goto(`${SITE}/p/1/${path}`);
    await page.locator(".compare [role=switch]").waitFor({ timeout: 15000 });
    await setCompare(false);
    await page.locator("main table").first().waitFor({ timeout: 15000 });
    await settle(800);
    const off = await compareMarks();
    assert.deepEqual(off, { chg: 0, prevCols: 0 }, `${path || "overview"}: 비교를 끄면 증감·이전 값이 없어야 한다`);
    await setCompare(true);
    await page.waitForFunction(() => [...document.querySelectorAll("main th")].some((t) => t.textContent.trim() === "이전 기간"), null, { timeout: 15000 });
    const on = await compareMarks();
    assert.ok(on.chg > 0, `${path || "overview"}: 변화 배지가 보여야 한다 ${JSON.stringify(on)}`);
    if (needCols) assert.ok(on.prevCols >= 2, `${path || "overview"}: 이전 기간·변화 열 ${JSON.stringify(on)}`);
    // 데스크톱에서는 진짜 표로 그린다 (머리글과 칸이 어긋나지 않게)
    const misaligned = await page.evaluate(() =>
      [...document.querySelectorAll("main table")].filter(
        (t) => getComputedStyle(t).display !== "table" || (t.tHead && t.tBodies[0] && Math.abs(t.tHead.offsetWidth - t.tBodies[0].offsetWidth) > 1),
      ).length,
    );
    assert.equal(misaligned, 0, `${path || "overview"}: 표 머리글·칸 정렬`);
    if (path === "") {
      // 지표 카드 증감 + 추이 차트의 이전 기간 점선·범례
      assert.ok(await page.locator(".stat .delta").count() >= 4);
      await page.locator('.chart-legend:has-text("이전 기간")').waitFor({ timeout: 15000 });
      await settle(800);
      await page.screenshot({ path: join(SHOTS, "demo-compare-overview.png") });
    }
    if (path === "revenue") {
      await page.locator('th:has-text("ROAS 변화")').waitFor();
      await settle(800);
      await page.screenshot({ path: join(SHOTS, "demo-compare-revenue.png") });
    }
  }
  // 다시 끄면 사라진다
  await setCompare(false);
  await page.waitForFunction(() => document.querySelectorAll("main .chg, main .delta").length === 0, null, { timeout: 15000 });
  // 기간이 의미 없는 화면에는 비교 토글이 없다
  for (const path of ["p/1/realtime", "p/1/settings?tab=install", "admin"]) {
    await page.goto(`${SITE}/${path}`);
    await page.locator("main h1").first().waitFor({ timeout: 15000 });
    assert.equal(await page.locator(".compare").count(), 0, `${path}: 비교 토글이 없어야 한다`);
  }
});

test("모든 리포트 화면에 예시 데이터가 보인다", async () => {
  // 사이드바 링크로 이동 (라우터가 기본 경로를 붙이는지 확인)
  for (const [label, path] of [["실시간", "realtime"], ["유입", "acquisition"], ["페이지", "pages"], ["이벤트", "events"], ["전자상거래", "funnel"]]) {
    await page.click(`.nav a:has-text("${label}")`);
    await page.waitForURL(`${SITE}/p/1/${path}`);
    await page.locator(".topbar").waitFor();
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
    await page.locator(".tabs").waitFor({ timeout: 15000 });
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

/** 모바일 화면 점검: 문서 가로 넘침, 세그먼트 줄바꿈, 상단 바 높이 */
async function mobileChecks(p, label) {
  return p.evaluate((label) => {
    const de = document.documentElement;
    const seg = [...document.querySelectorAll(".segmented button")]
      .filter((b) => b.offsetParent !== null)
      .map((b) => ({ t: b.textContent.trim(), h: b.offsetHeight, lh: parseFloat(getComputedStyle(b).lineHeight) || 20, sh: b.scrollHeight }))
      .filter((b) => b.sh > b.h + 1 || b.h > 48);
    return { label, sw: de.scrollWidth, cw: de.clientWidth, top: document.querySelector(".topbar")?.offsetHeight ?? 0, wrapped: seg.map((b) => b.t) };
  }, label);
}

test("모바일(iPhone 13): 가로 넘침·잘림 없이 모든 화면이 보인다", async () => {
  const { devices } = playwright();
  const ctx = await browser.newContext({ ...devices["iPhone 13"], locale: "ko-KR", timezoneId: "Asia/Seoul" });
  const m = await ctx.newPage();
  m.on("pageerror", (e) => errors.push(`mobile pageerror: ${e.message}`));
  await m.goto(`${SITE}/login`);
  await m.click('button:has-text("로그인")');
  await m.waitForURL(new RegExp(`${BASE}/p/1(/|$)`));
  // 데모 알림: 뜬 뒤 저절로 사라진다 (닫기 버튼도 있다)
  const toast = m.locator(".toast");
  await toast.waitFor({ timeout: 5000 });
  assert.equal(await m.locator('.toast button[aria-label="알림 닫기"]').count(), 1);
  await toast.waitFor({ state: "detached", timeout: 8000 });

  const pages = [
    ["overview", ""], ["realtime", "realtime"], ["acquisition", "acquisition"], ["pages", "pages"], ["events", "events"],
    ["funnel", "funnel"], ["revenue", "revenue"], ["settings-install", "settings?tab=install"], ["settings-tracking", "settings?tab=tracking"],
    ["settings-rules", "settings?tab=rules"], ["settings-attribution", "settings?tab=attribution"], ["settings-spend", "settings?tab=spend"],
  ];
  for (const [name, path] of pages) {
    await m.goto(`${SITE}/p/1/${path}`);
    await m.locator("main h1").first().waitFor({ timeout: 15000 });
    await m.waitForTimeout(1200);
    const r = await mobileChecks(m, name);
    assert.ok(r.sw <= r.cw, `${name}: 문서 가로 넘침 ${r.sw} > ${r.cw}`);
    assert.deepEqual(r.wrapped, [], `${name}: 세그먼트 글자가 줄바꿈됨`);
    assert.ok(r.top <= 120, `${name}: 상단 바 높이 ${r.top}`);
    await m.screenshot({ path: join(SHOTS, `demo-mobile-${name}.png`), fullPage: true });
  }
  // 상단 바: 메뉴·프로젝트 이름·기간·비교가 한 화면 안에 (잘리지 않음)
  await m.goto(`${SITE}/p/1/revenue`);
  await m.locator(".compare [role=switch]").waitFor({ timeout: 15000 });
  for (const sel of [".menu-btn", ".topbar-title", ".compare [role=switch]"]) {
    const b = await m.locator(sel).boundingBox();
    assert.ok(b && b.x >= 0 && b.x + b.width <= 390 + 0.5, `${sel} 잘림: ${JSON.stringify(b)}`);
  }
  // 기간 버튼은 가로 스크롤로 모두 닿는다 ('직접 선택')
  await m.locator('.range-picker [role=radio]:has-text("직접 선택")').scrollIntoViewIfNeeded();
  assert.ok(await m.locator('.range-picker [role=radio]:has-text("직접 선택")').isVisible());

  // 비교를 켜면 모바일 카드 행의 주 지표 옆에 변화 배지가 보인다
  await m.locator(".compare [role=switch]").click();
  await m.waitForFunction(() => [...document.querySelectorAll(".m-only")].some((e) => e.offsetParent !== null), null, { timeout: 15000 });
  const r = await mobileChecks(m, "revenue-compare");
  assert.ok(r.sw <= r.cw, `비교 켠 매출 화면 가로 넘침 ${r.sw}`);
  // 매체별 표는 모바일에서 매체·기여 매출·비중·ROAS만
  const visibleLabels = await m.evaluate(() =>
    [...document.querySelector("main .table.table-cards tbody tr").children].filter((td) => td.offsetParent !== null).map((td) => td.dataset.label),
  );
  assert.deepEqual(visibleLabels, ["매체", "기여 매출", "비중", "ROAS"]);
  await m.screenshot({ path: join(SHOTS, "demo-mobile-revenue-compare.png"), fullPage: true });
  await m.goto(`${SITE}/p/1/`);
  await m.locator(".stat .delta").first().waitFor({ timeout: 15000 });
  await m.waitForTimeout(800);
  await m.screenshot({ path: join(SHOTS, "demo-mobile-overview-compare.png"), fullPage: true });
  await m.locator(".compare [role=switch]").click();

  // 드로어: 모바일에서 전체 너비
  await m.goto(`${SITE}/p/1/events`);
  const row = m.locator("tr.clickable").first();
  await row.waitFor({ timeout: 15000 });
  await row.click();
  const drawer = m.locator("[role=dialog]");
  await drawer.waitFor();
  const db = await drawer.boundingBox();
  assert.ok(db.width >= 389, `드로어 너비 ${db.width}`);
  await m.waitForTimeout(600);
  await m.screenshot({ path: join(SHOTS, "demo-mobile-events-drawer.png") });
  await m.keyboard.press("Escape");

  // 360px·768px에서도 문서 가로 넘침이 없다
  for (const width of [360, 768]) {
    await m.setViewportSize({ width, height: 800 });
    for (const [name, path] of pages) {
      await m.goto(`${SITE}/p/1/${path}`);
      await m.locator("main h1").first().waitFor({ timeout: 15000 });
      await m.waitForTimeout(700);
      const r = await mobileChecks(m, `${name}@${width}`);
      assert.ok(r.sw <= r.cw, `${name}@${width}: 문서 가로 넘침 ${r.sw} > ${r.cw}`);
      assert.deepEqual(r.wrapped, [], `${name}@${width}: 세그먼트 줄바꿈`);
    }
  }
  await ctx.close();
});

test("콘솔·페이지 오류가 없다", () => {
  assert.deepEqual(errors, []);
});

// E2E 공용 도우미: 프로세스 기동, API 호출, 대기.
import { spawn, execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

export const ROOT = resolve(new URL("..", import.meta.url).pathname);
const TARGET = process.env.CARGO_TARGET_DIR || join(ROOT, "target");
const BIN = (name) => join(TARGET, "debug", name);

export const DATABASE_URL = process.env.DATABASE_URL || "postgres://analytics:analytics@localhost:5432/analytics";
export const CLICKHOUSE_URL = process.env.CLICKHOUSE_URL || "http://localhost:8123";
export const INGEST = "http://localhost:18081";
export const API = "http://localhost:18082";
export const DASHBOARD_ORIGIN = "http://localhost:8080";

// 헤드리스 표시가 없는 일반 브라우저 UA (봇 필터 통과용)
export const UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

export function playwright() {
  // 사전 설치된 playwright (어느 디렉터리에서나 require 가능)
  return createRequire(import.meta.url)("playwright");
}

const baseEnv = () => ({ ...process.env, DATABASE_URL, CLICKHOUSE_URL, RUST_LOG: process.env.RUST_LOG || "warn" });

export function build() {
  execFileSync("cargo", ["build", "-q", "-p", "analytics-ingest", "-p", "analytics-api", "-p", "analytics-jobs"], {
    cwd: ROOT,
    stdio: "inherit",
  });
  execFileSync("npm", ["run", "-s", "build"], { cwd: join(ROOT, "tracker"), stdio: "inherit" });
}

export function jobs(args) {
  return execFileSync(BIN("analytics-jobs"), args, { cwd: ROOT, env: baseEnv(), encoding: "utf8" });
}

function start(name, env) {
  const p = spawn(BIN(name), [], { cwd: ROOT, env: { ...baseEnv(), ...env }, stdio: ["ignore", "inherit", "inherit"] });
  return p;
}

export async function startStack() {
  const archive = mkdtempSync(join(tmpdir(), "e2e-archive-"));
  const procs = [
    start("analytics-ingest", {
      EDGE_EMULATION: "true",
      INGEST_BIND: "127.0.0.1:18081",
      ARCHIVE_DIR: archive,
      FLUSH_INTERVAL_MS: "200",
      TRACKER_DIST: join(ROOT, "tracker/dist"),
      DEV_SITE_DIR: join(ROOT, "dev/site"),
    }),
    start("analytics-api", { API_BIND: "127.0.0.1:18082", DASHBOARD_ORIGINS: DASHBOARD_ORIGIN }),
  ];
  await waitFor(async () => (await fetch(`${INGEST}/healthz`)).ok, "ingest");
  await waitFor(async () => (await fetch(`${API}/healthz`)).ok, "api");
  return () => procs.forEach((p) => p.kill("SIGTERM"));
}

export async function waitFor(fn, label, timeoutMs = 30000) {
  const until = Date.now() + timeoutMs;
  let last;
  while (Date.now() < until) {
    try {
      if (await fn()) return;
    } catch (e) {
      last = e;
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`${label} 대기 시간 초과 ${last ? `(${last.message})` : ""}`);
}

/** 대시보드처럼 쿠키 세션으로 API를 호출하는 클라이언트 */
export class ApiClient {
  cookie = "";
  async call(method, path, body) {
    const headers = { origin: DASHBOARD_ORIGIN, cookie: this.cookie };
    if (body !== undefined) headers["content-type"] = "application/json";
    const res = await fetch(`${API}${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    const set = res.headers.getSetCookie?.() || [];
    for (const c of set) if (/at_session=/.test(c)) this.cookie = c.split(";")[0];
    const text = await res.text();
    const json = text ? JSON.parse(text) : null;
    if (!res.ok) throw new Error(`${method} ${path} → ${res.status} ${text}`);
    return json;
  }
}

/** Asia/Seoul 기준 날짜 YYYY-MM-DD */
export function kstDate(ms) {
  return new Date(ms + 9 * 3600 * 1000).toISOString().slice(0, 10);
}

export async function clickhouse(sql) {
  const res = await fetch(`${CLICKHOUSE_URL}/?database=analytics`, { method: "POST", body: sql });
  return (await res.text()).trim();
}

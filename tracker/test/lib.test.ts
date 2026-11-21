// tracker/src/lib.ts 순수 로직 테스트. `npm test`가 esbuild로 묶어 node --test로 돌린다.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  CFG_MAX_AGE_MS, FLUSH_AT, IDLE_MS, MAX_BODY_BYTES, MAX_EVENTS,
  bytes, campaignSig, needsConfig, newSession, shouldFlush, split,
} from "../src/lib";

const DAY = 24 * 60 * 60 * 1000;

test("정책 상수", () => {
  assert.equal(FLUSH_AT, 10);
  assert.equal(IDLE_MS, 15_000);
  assert.equal(MAX_EVENTS, 50);
  assert.ok(MAX_BODY_BYTES <= 60_000, "서버 상한(64KB) 아래");
  assert.equal(CFG_MAX_AGE_MS, DAY);
});

test("10건이 쌓이면 바로 보내고, 그 전에는 유휴 타이머를 기다린다", () => {
  for (let n = 0; n < 10; n++) assert.equal(shouldFlush(n), false, String(n));
  assert.equal(shouldFlush(10), true);
  assert.equal(shouldFlush(11), true);
});

test("UTF-8 바이트 길이", () => {
  assert.equal(bytes("abc"), 3);
  assert.equal(bytes("한"), 3);
  assert.equal(bytes("é"), 2);
  assert.equal(bytes("😀"), 4);
});

const env = (e: unknown[]) => JSON.stringify({ k: "pk_x", s: "sid", e });

test("배치 분할: 50건 이하로 나눈다", () => {
  const items = Array.from({ length: 120 }, (_, i) => ({ i }));
  const b = split(items, env);
  assert.deepEqual(b.map((x) => x.length), [50, 50, 20]);
  assert.deepEqual(b.flat(), items, "순서 유지, 빠짐 없음");
});

test("배치 분할: 각 본문은 60KB 미만 (413 금지)", () => {
  const big = "가".repeat(5_000); // 15KB
  const items = Array.from({ length: 9 }, (_, i) => ({ i, big }));
  const b = split(items, env);
  for (const x of b) assert.ok(bytes(env(x)) < MAX_BODY_BYTES, String(bytes(env(x))));
  assert.deepEqual(b.flat(), items);
  assert.ok(b.length >= 3);
  // 계산한 크기가 실제 본문 크기와 정확히 같다 (경계값)
  const one = [{ s: "x".repeat(100) }];
  const exact = bytes(env(one));
  assert.equal(split(one, env, exact).length, 1);
  assert.equal(split(one, env, exact - 1).length, 0);
});

test("배치 분할: 혼자서도 한도를 넘는 이벤트는 버린다", () => {
  const items = [{ a: 1 }, { huge: "x".repeat(70_000) }, { a: 2 }];
  assert.deepEqual(split(items, env), [[{ a: 1 }, { a: 2 }]]);
  assert.deepEqual(split([], env), []);
});

test("설정 갱신 규칙: 캐시 없음·24시간 경과·버전 불일치일 때만", () => {
  const now = 1_800_000_000_000;
  assert.equal(needsConfig(null, now), true, "캐시 없음");
  assert.equal(needsConfig({ v: 3 }, now), true, "받은 시각 없음 (예전 형식)");
  assert.equal(needsConfig({ v: 3, at: now - 1000 }, now), false);
  assert.equal(needsConfig({ v: 3, at: now - DAY + 1 }, now), false);
  assert.equal(needsConfig({ v: 3, at: now - DAY }, now), true, "24시간");
  assert.equal(needsConfig({ v: 3, at: now }, now, "3"), false, "같은 버전");
  assert.equal(needsConfig({ v: 3, at: now }, now, "4"), true, "버전 변경");
  assert.equal(needsConfig({ v: 3, at: now }, now, null), false, "헤더 없음");
  assert.equal(needsConfig({ v: 3, at: now }, now, ""), false);
});

test("캠페인 서명", () => {
  assert.equal(campaignSig(""), undefined);
  assert.equal(campaignSig("?a=1&b=2"), undefined);
  assert.equal(campaignSig("?utm_source=naver&utm_medium=cpc"), "naver|cpc||||||");
  assert.equal(campaignSig("?gclid=abc"), "|||abc||||");
  assert.notEqual(campaignSig("?utm_source=naver"), campaignSig("?utm_source=meta"));
});

test("세션: 30분 무활동·캠페인 변경이면 새 세션", () => {
  const now = 1_800_000_000_000;
  const MIN = 60_000;
  const raw = (last: number) => "sid1." + last;
  assert.equal(newSession(undefined, now, undefined, null), true, "쿠키 없음");
  assert.equal(newSession(raw(now - 10 * MIN), now, undefined, null), false);
  assert.equal(newSession(raw(now - 30 * MIN - 1), now, undefined, null), true, "30분 초과");
  assert.equal(newSession(raw(now - 30 * MIN), now, undefined, null), false, "정확히 30분은 유지");
  assert.equal(newSession(raw(now - MIN), now, "a|b", "a|b"), false, "같은 캠페인");
  assert.equal(newSession(raw(now - MIN), now, "a|c", "a|b"), true, "다른 캠페인");
  assert.equal(newSession(raw(now - MIN), now, undefined, "a|b"), false, "캠페인 없는 페이지는 유지");
  assert.equal(newSession("garbage", now, undefined, null), true, "깨진 쿠키");
});

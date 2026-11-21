/**
 * 트래커 순수 로직 (DOM·저장소 없음). t.ts가 묶어 쓰고, test/lib.test.ts가 node로 검사한다.
 */

/** 이만큼 쌓이면 바로 보낸다. */
export const FLUSH_AT = 10;
/** 마지막 이벤트 뒤 이만큼 조용하면 보낸다. 페이지를 떠날 때는 sendBeacon으로 즉시. */
export const IDLE_MS = 15_000;
/** 요청 1건의 이벤트 수 상한 (서버 MAX_BATCH_EVENTS). */
export const MAX_EVENTS = 50;
/** 요청 본문 상한. 서버 상한(64KB)보다 작게 잡아 413을 받지 않는다. */
export const MAX_BODY_BYTES = 60_000;
/** 캐시한 설정을 이 시간이 지나면 다시 받는다 (버전 변경은 x-cfg-v로 더 빨리 안다). */
export const CFG_MAX_AGE_MS = 864e5;
/** 30분 무활동이면 새 세션. */
export const SESSION_MS = 18e5;

/** UTF-8 바이트 길이 (짝 없는 서로게이트는 크게 센다: 본문 상한 쪽으로 안전). */
export const bytes = (s: string): number => {
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    n += c < 128 ? 1 : c < 2048 ? 2 : c >= 0xd800 && c < 0xdc00 ? (i++, 4) : 3;
  }
  return n;
};

/**
 * 순서를 지키며 배치로 나눈다: 배치마다 `encode(batch)`가 `max` 바이트 이하이고 `maxEvents`건 이하.
 * `encode`는 이벤트 배열을 `JSON.stringify`로 그대로 넣어야 한다 (크기를 이벤트별로 더해 계산한다).
 * 혼자서도 한도를 넘는 이벤트는 버린다 (보내도 413).
 */
export const split = <T>(items: T[], encode: (batch: T[]) => string, max = MAX_BODY_BYTES, maxEvents = MAX_EVENTS): T[][] => {
  const base = bytes(encode([]));
  const out: T[][] = [];
  let cur: T[] = [];
  let size = base;
  for (const it of items) {
    const n = bytes(JSON.stringify(it));
    if (base + n > max) continue;
    if (cur.length && (cur.length >= maxEvents || size + 1 + n > max)) {
      out.push(cur);
      cur = [];
      size = base;
    }
    size += (cur.length ? 1 : 0) + n;
    cur.push(it);
  }
  if (cur.length) out.push(cur);
  return out;
};

/** 큐 길이로 즉시 전송 여부. 아니면 유휴 타이머(IDLE_MS)를 다시 건다. */
export const shouldFlush = (queued: number): boolean => queued >= FLUSH_AT;

/**
 * 설정을 다시 받아야 하는지: 캐시가 없거나, 받은 지 24시간이 지났거나,
 * 수집 응답의 `x-cfg-v`가 캐시한 버전과 다를 때.
 */
export const needsConfig = (cached: { v: number; at?: number } | null, now: number, header?: string | null): boolean =>
  !cached || !(now - (cached.at || 0) < CFG_MAX_AGE_MS) || (!!header && header !== String(cached.v));

const CAMPAIGN_KEYS = ["utm_source", "utm_medium", "utm_campaign", "gclid", "fbclid", "abx_tid", "n_keyword", "click_id"];

/** URL 쿼리의 캠페인 서명. 캠페인 파라미터가 하나도 없으면 `undefined`. */
export const campaignSig = (search: string): string | undefined => {
  const p = new URLSearchParams(search);
  const sig = CAMPAIGN_KEYS.map((k) => p.get(k) || "").join("|");
  return /[^|]/.test(sig) ? sig : undefined;
};

/**
 * 새 세션인지. `raw`는 `_at_sid` 쿠키(`{id}.{마지막 활동 ms}`).
 * 쿠키가 없거나 깨졌거나, 30분 넘게 무활동이거나, 캠페인이 바뀌었으면 새 세션.
 */
export const newSession = (raw: string | undefined, now: number, sig: string | undefined, prevSig: string | null): boolean => {
  const [id, last] = raw ? raw.split(".") : [];
  return !id || !(now - +last <= SESSION_MS) || (!!sig && sig !== prevSig);
};

/**
 * 22-analytics 트래커 (t.js)
 *
 * <script async src="https://collect.<agency-domain>/t.js" data-key="pk_..."></script>
 * <script>window.at=window.at||function(){(at.q=at.q||[]).push(arguments)};</script>
 *
 * at('event', 'signup_complete', { plan: 'pro' })
 * at('ecommerce', 'purchase', { order_id, value, currency, items: [{ id, name, price, qty }] })
 * at('identify', sha256Hex(회원ID))
 * at('consent', false)
 * at('pageview')
 * at('getVisitorId', (id) => ...)
 *
 * 전송: 10건이 쌓이거나 15초 조용하면 보내고, 페이지를 떠날 때는 sendBeacon으로 즉시 보낸다.
 * 요청마다 50건·60KB 이하로 나눈다. 설정(/v1/config)은 캐시가 없거나 24시간이 지났거나
 * 수집 응답의 x-cfg-v가 캐시 버전과 다를 때만 받는다 (페이지뷰당 엣지 요청 ≈ 1건).
 */

import { IDLE_MS, SESSION_MS, campaignSig, needsConfig, newSession, shouldFlush, split } from "./lib";

type Val = string | number | boolean | null;
type Props = Record<string, Val>;

interface Item { id?: string; name?: string; price?: number; qty?: number }
interface Ec { order_id?: string; value?: number; currency?: string; items?: Item[] }

/** 서버 PublicConfig와 같은 키 (+ 받은 시각 at) */
interface Cfg { v: number; pv: boolean; spa: boolean; out: boolean; dl: boolean; scr: boolean; dnt: boolean; at?: number }

interface WireEvent {
  id: string; n: string; t: number; u: string;
  r?: string; ti?: string; sw?: number; ss?: boolean; p?: Props; ec?: Ec;
}

((w: Window & { at?: any }, d: Document) => {
  const script = d.currentScript as HTMLScriptElement | null;
  const key = script && script.getAttribute("data-key");
  if (!script || !key) return;

  const loc = location;
  const base = new URL(script.src).origin;
  const CFG_KEY = "_at_cfg_" + key;
  const DEFAULTS: Cfg = { v: 0, pv: true, spa: true, out: false, dl: false, scr: false, dnt: false };

  // ---- 저장소 도우미 ------------------------------------------------------
  const ls = {
    get(k: string): string | null { try { return localStorage.getItem(k); } catch { return null; } },
    set(k: string, v: string | null) { try { v === null ? localStorage.removeItem(k) : localStorage.setItem(k, v); } catch { /* 사파리 사생활 보호 모드 */ } },
  };

  const getCookie = (name: string): string | undefined => {
    const m = d.cookie.match(new RegExp("(?:^|; )" + name + "=([^;]*)"));
    return m ? decodeURIComponent(m[1]) : undefined;
  };

  /** 쿠키를 둘 수 있는 가장 상위 도메인을 찾는다 (www.shop.co.kr → .shop.co.kr). */
  let rootDomain: string | undefined;
  const setCookie = (name: string, value: string, maxAgeSec: number) => {
    const base = name + "=" + encodeURIComponent(value) + "; path=/; max-age=" + maxAgeSec + "; SameSite=Lax" + (loc.protocol === "https:" ? "; Secure" : "");
    if (rootDomain === undefined) {
      const parts = loc.hostname.split(".");
      rootDomain = "";
      for (let i = parts.length - 2; i >= 0; i--) {
        const dom = parts.slice(i).join(".");
        d.cookie = base + "; domain=" + dom;
        if (getCookie(name) === value) { rootDomain = dom; return; }
      }
    }
    d.cookie = base + (rootDomain ? "; domain=" + rootDomain : "");
  };

  const uuid = (): string => {
    const c = w.crypto;
    if (c && c.randomUUID) return c.randomUUID();
    const b = new Uint8Array(16);
    c.getRandomValues(b);
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    const h = Array.from(b, (x) => (x + 256).toString(16).slice(1)).join("");
    return h.slice(0, 8) + "-" + h.slice(8, 12) + "-" + h.slice(12, 16) + "-" + h.slice(16, 20) + "-" + h.slice(20);
  };

  // ---- 상태 ---------------------------------------------------------------
  let cached: Cfg | null = null;
  try { cached = JSON.parse(ls.get(CFG_KEY) || "null"); } catch { /* 깨진 캐시 무시 */ }
  const cfg: Cfg = { ...DEFAULTS, ...cached };
  let off = ls.get("_at_off") === "1" || (cfg.dnt && navigator.doNotTrack === "1");
  let member = ls.get("_at_m");

  let vid = getCookie("_at_cv");
  if (!vid) { vid = uuid(); }
  setCookie("_at_cv", vid, 31536000);

  let sid = "";
  let fresh = false;
  /** 세션 랜딩 정보 { s: sid, u: 랜딩 URL, r: 랜딩 referrer } */
  let landing: { s: string; u: string; r?: string } | null = null;
  try { landing = JSON.parse(ls.get("_at_l") || "null"); } catch { /* 무시 */ }
  /** 세션 확인·갱신. 30분 무활동이거나 캠페인이 바뀌면 새 세션. */
  const touchSession = (campaignSig?: string) => {
    const now = Date.now();
    const raw = getCookie("_at_sid");
    if (newSession(raw, now, campaignSig, ls.get("_at_cs"))) {
      sid = uuid();
      fresh = true;
      ls.set("_at_cs", campaignSig || null);
    } else {
      sid = raw!.split(".")[0];
    }
    setCookie("_at_sid", sid + "." + now, SESSION_MS / 1000);
  };

  // ---- 설정 ------------------------------------------------------------------
  let cfgBusy = false;
  /** 다음 페이지에서 쓸 설정 갱신 (필요할 때만, 페이지당 최대 1회). */
  const refreshCfg = (header?: string | null) => {
    if (cfgBusy || !needsConfig(cached, Date.now(), header)) return;
    cfgBusy = true;
    // 버전을 쿼리에 넣어 HTTP·엣지 캐시의 이전 버전 응답을 피한다
    fetch(base + "/v1/config?k=" + encodeURIComponent(key) + (header ? "&v=" + encodeURIComponent(header) : ""))
      .then((r) => (r.ok ? r.json() : null))
      .then((j) => { if (j) { cached = { ...j, at: Date.now() }; ls.set(CFG_KEY, JSON.stringify(cached)); } })
      .catch(() => { /* 무시 */ });
  };

  // ---- 전송 큐 --------------------------------------------------------------
  let queue: WireEvent[] = [];
  let timer: ReturnType<typeof setTimeout> | undefined;

  const flush = (unloading?: boolean) => {
    if (timer) { clearTimeout(timer); timer = undefined; }
    const l = landing && landing.s === sid ? { u: landing.u, r: landing.r } : undefined;
    const encode = (e: WireEvent[]) => JSON.stringify({ k: key, v: vid, s: sid, m: member || undefined, sv: 1, l, e });
    const url = base + "/v1/collect";
    for (const batch of split(queue, encode)) {
      const body = encode(batch);
      // 문자열 본문 = text/plain → CORS preflight 없음
      if (unloading && navigator.sendBeacon && navigator.sendBeacon(url, body)) continue;
      fetch(url, { method: "POST", body, keepalive: true, credentials: "include" })
        .then((r) => refreshCfg(r.headers.get("x-cfg-v")))
        .catch(() => { /* 다음 배치에서 재시도하지 않음 */ });
    }
    queue = [];
  };

  const push = (n: string, extra?: Partial<WireEvent>) => {
    if (off) return;
    touchSession();
    const e: WireEvent = { id: uuid(), n, t: Date.now(), u: loc.href, sw: screen.width, ...extra };
    if (fresh) {
      e.ss = true;
      fresh = false;
      landing = { s: sid, u: e.u, r: e.r };
      ls.set("_at_l", JSON.stringify(landing));
    }
    queue.push(e);
    if (shouldFlush(queue.length)) flush();
    else { clearTimeout(timer); timer = setTimeout(flush, IDLE_MS); }
  };

  // ---- 페이지뷰 ------------------------------------------------------------
  let lastUrl = "";
  let maxScroll = 0;

  const pageview = () => {
    const href = loc.href;
    if (href === lastUrl) return;
    const ref = lastUrl || d.referrer;
    lastUrl = href;
    maxScroll = 0;
    if (off) return;
    touchSession(campaignSig(loc.search));
    push("page_view", { r: ref, ti: d.title });
  };

  // ---- 명령 처리 ------------------------------------------------------------
  const cmd = (name: string, a?: any, b?: any) => {
    switch (name) {
      case "event": push(String(a), { p: b }); break;
      case "pageview": lastUrl = ""; pageview(); break;
      case "ecommerce": push(String(a), { ec: b }); break;
      case "identify":
        member = /^[0-9a-f]{64}$/i.test(a) ? String(a).toLowerCase() : null;
        ls.set("_at_m", member);
        break;
      case "consent":
        off = a === false;
        ls.set("_at_off", off ? "1" : null);
        if (off) { queue = []; setCookie("_at_cv", "", 0); setCookie("_at_sid", "", 0); ls.set("_at_m", null); }
        break;
      case "getVisitorId": if (typeof a === "function") a(vid); break;
    }
  };

  const pending: ArrayLike<any>[] = (w.at && w.at.q) || [];
  w.at = (...args: any[]) => cmd(args[0], args[1], args[2]);

  // ---- 자동 수집 ------------------------------------------------------------
  if (cfg.spa) {
    const wrap = (fn: "pushState" | "replaceState") => {
      const orig = history[fn];
      history[fn] = function (this: History, ...args: any[]) {
        const r = orig.apply(this, args as any);
        pageview();
        return r;
      };
    };
    wrap("pushState");
    wrap("replaceState");
    w.addEventListener("popstate", pageview);
  }

  if (cfg.out || cfg.dl) {
    d.addEventListener("click", (ev) => {
      const a = (ev.target as Element | null)?.closest?.("a") as HTMLAnchorElement | null;
      if (!a || !a.href) return;
      const ext = (a.pathname.match(/\.(pdf|zip|xlsx?|docx?|pptx?|hwp|csv|dmg|exe|apk)$/i) || [])[1];
      if (cfg.dl && ext) push("file_download", { p: { url: a.href, ext: ext.toLowerCase() } });
      else if (cfg.out && a.host && a.host !== loc.host) push("outbound_click", { p: { url: a.href } });
    }, true);
  }

  if (cfg.scr) {
    w.addEventListener("scroll", () => {
      const h = d.documentElement.scrollHeight - innerHeight;
      if (h <= 0) return;
      const pct = (scrollY / h) * 100;
      for (const m of [25, 50, 75, 90]) {
        if (pct >= m && maxScroll < m) { maxScroll = m; push("scroll", { p: { percent: m } }); }
      }
    }, { passive: true });
  }

  const hide = () => flush(true);
  d.addEventListener("visibilitychange", () => d.visibilityState === "hidden" && hide());
  w.addEventListener("pagehide", hide);

  if (cfg.pv) pageview();
  for (let i = 0; i < pending.length; i++) {
    const args = pending[i];
    cmd(args[0], args[1], args[2]);
  }

  refreshCfg();
})(window, document);

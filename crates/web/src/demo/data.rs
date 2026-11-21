//! 데모 데이터 생성기 (결정적: 프로젝트 id·날짜·순번으로 시드한 해시 난수).
//!
//! - 오늘(Asia/Seoul) 기준 최근 [`HISTORY_DAYS`]일만 데이터가 있다. 오래된 날·미래는 0.
//! - 오늘은 현재 시각까지의 시간대 비중만큼만 쌓인다 (`now_ms`를 받는다).
//! - 성장 추세 + 요일 계절성(쇼핑몰은 주말이 높다) + 프로모션(블랙프라이데이) 급증.
//! - 합계가 맞도록 정수 배분은 최대 잔여법을 쓴다 (채널·페이지 합 = 전체).

use crate::logic::dates::Day;
use analytics_core::attribution::{DAY_MS, Touch};

/// 데이터가 있는 날 수 (오늘 포함)
pub const HISTORY_DAYS: i32 = 120;
pub const MINUTE_MS: i64 = 60_000;
pub const SEOUL_MS: i64 = 9 * 3_600_000;

/// 채널 (세션 범위). 순서는 [`Profile::channels`]와 같다.
pub const CHANNELS: [&str; 9] = [
    "naver_sa",
    "meta",
    "google_ads",
    "kakao",
    "criteo",
    "organic_search",
    "organic_social",
    "referral",
    "direct",
];
/// 광고비가 있는 채널 수 (앞에서부터)
pub const PAID: usize = 5;

/// 채널별 캠페인 (광고비·접점)
pub fn campaigns(ch: &str) -> &'static [&'static str] {
    match ch {
        "naver_sa" => &["브랜드검색", "패딩_키워드"],
        "meta" => &["BF_리타겟팅", "신규_관심사"],
        "google_ads" => &["PMax_전체", "검색_브랜드"],
        "kakao" => &["카카오모먼트_BF", "비즈보드"],
        "criteo" => &["다이나믹_리타겟팅", "신규고객"],
        _ => &[""],
    }
}

/// 시간대별 트래픽 비중 (0~23시, 저녁이 높다)
const HOURLY: [f64; 24] = [
    2.6, 1.6, 1.0, 0.7, 0.6, 0.8, 1.5, 2.6, 3.6, 4.2, 4.6, 4.9, 5.4, 5.2, 4.9, 4.8, 4.9, 5.0, 5.3,
    5.9, 6.6, 7.1, 6.6, 4.4,
];

// ---------------------------------------------------------------- 난수

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 여러 값으로 만든 시드
pub fn seed(parts: &[i64]) -> u64 {
    parts.iter().fold(0x5EED_u64, |a, p| mix(a ^ (*p as u64)))
}

/// [0, 1)
pub fn unit(parts: &[i64]) -> f64 {
    (seed(parts) >> 11) as f64 / (1u64 << 53) as f64
}

/// 가중치 중 하나 고르기
fn pick(weights: &[f64], u: f64) -> usize {
    let total: f64 = weights.iter().sum();
    let mut acc = 0.0;
    for (i, w) in weights.iter().enumerate() {
        acc += w / total;
        if u < acc {
            return i;
        }
    }
    weights.len() - 1
}

/// 최대 잔여법: 합이 정확히 `total`이 되도록 가중치대로 나눈다.
pub fn split(total: u64, weights: &[f64]) -> Vec<u64> {
    let sum: f64 = weights.iter().sum();
    if total == 0 || sum <= 0.0 {
        return vec![0; weights.len()];
    }
    let exact: Vec<f64> = weights.iter().map(|w| total as f64 * w / sum).collect();
    let mut out: Vec<u64> = exact.iter().map(|x| x.floor() as u64).collect();
    let mut rest = total - out.iter().sum::<u64>();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|a, b| {
        let fa = exact[*a] - exact[*a].floor();
        let fb = exact[*b] - exact[*b].floor();
        fb.total_cmp(&fa).then(a.cmp(b))
    });
    for i in order {
        if rest == 0 {
            break;
        }
        out[i] += 1;
        rest -= 1;
    }
    out
}

// ---------------------------------------------------------------- 프로젝트 성격

pub struct PathDef {
    pub path: &'static str,
    pub title: &'static str,
    /// 페이지뷰 비중
    pub pv: f64,
    /// 랜딩(세션 시작) 비중
    pub landing: f64,
}

const fn pd(path: &'static str, title: &'static str, pv: f64, landing: f64) -> PathDef {
    PathDef {
        path,
        title,
        pv,
        landing,
    }
}

pub struct Item {
    pub id: &'static str,
    pub name: &'static str,
    pub price: i64,
}

pub struct Profile {
    /// 하루 세션 (성장 끝, 평일 기준)
    pub base: f64,
    /// 주말 배수
    pub weekend: f64,
    /// 세션당 페이지뷰
    pub ppv: f64,
    /// 세션당 주문
    pub conv: f64,
    /// 객단가
    pub aov: f64,
    /// [`CHANNELS`] 순서의 세션 비중
    pub channels: [f64; 9],
    /// 접점 비중 (direct 제외: 직접 방문은 접점이 아니다)
    pub touch: [f64; 8],
    /// 광고 클릭당 비용 (원)
    pub cpc: [f64; PAID],
    pub paths: &'static [PathDef],
    pub items: &'static [Item],
}

const SHOP_PATHS: &[PathDef] = &[
    pd("/", "바이너리큐브 데모 쇼핑몰", 0.20, 0.34),
    pd("/category/outer", "아우터", 0.08, 0.05),
    pd("/goods/winter-padding", "윈터 롱패딩", 0.085, 0.10),
    pd("/event/black-friday", "블랙프라이데이 최대 70%", 0.05, 0.09),
    pd("/search", "검색 결과", 0.06, 0.01),
    pd("/cart", "장바구니", 0.06, 0.005),
    pd("/goods/cashmere-knit", "캐시미어 니트", 0.055, 0.07),
    pd("/category/knit", "니트", 0.05, 0.03),
    pd("/goods/wool-coat", "울 핸드메이드 코트", 0.045, 0.06),
    pd("/order/checkout", "주문서 작성", 0.035, 0.0),
    pd("/goods/down-vest", "경량 다운 베스트", 0.03, 0.04),
    pd("/mypage", "마이페이지", 0.03, 0.02),
    pd("/login", "로그인", 0.028, 0.01),
    pd("/goods/fleece-jacket", "플리스 자켓", 0.025, 0.035),
    pd("/order/complete", "주문 완료", 0.02, 0.0),
    pd("/event/coupon", "쿠폰존", 0.02, 0.03),
    pd("/review", "상품 후기", 0.02, 0.01),
    pd("/signup", "회원가입", 0.015, 0.005),
    pd(
        "/goods/winter-padding?color=black",
        "윈터 롱패딩 (블랙)",
        0.012,
        0.02,
    ),
    pd("/brand/story", "브랜드 스토리", 0.008, 0.01),
];

const BRAND_PATHS: &[PathDef] = &[
    pd("/", "바이너리큐브 데모 브랜드몰", 0.22, 0.36),
    pd("/collection/fw26", "26 FW 컬렉션", 0.10, 0.10),
    pd("/goods/signature-bag", "시그니처 레더 백", 0.09, 0.12),
    pd(
        "/event/black-friday",
        "블랙프라이데이 프라이빗 세일",
        0.05,
        0.08,
    ),
    pd("/goods/silk-scarf", "실크 스카프", 0.06, 0.07),
    pd("/goods/leather-wallet", "레더 지갑", 0.055, 0.06),
    pd("/cart", "장바구니", 0.055, 0.005),
    pd("/brand/story", "브랜드 스토리", 0.05, 0.05),
    pd("/lookbook", "룩북", 0.05, 0.04),
    pd("/search", "검색 결과", 0.04, 0.01),
    pd("/order/checkout", "주문서 작성", 0.035, 0.0),
    pd("/store", "매장 안내", 0.03, 0.03),
    pd("/mypage", "마이페이지", 0.03, 0.02),
    pd("/login", "로그인", 0.025, 0.01),
    pd("/order/complete", "주문 완료", 0.02, 0.0),
    pd("/goods/cashmere-knit", "캐시미어 니트", 0.02, 0.03),
    pd("/signup", "회원가입", 0.012, 0.005),
    pd("/event/coupon", "멤버십 쿠폰", 0.01, 0.01),
];

const SHOP_ITEMS: &[Item] = &[
    Item {
        id: "P-1001",
        name: "윈터 롱패딩",
        price: 189_000,
    },
    Item {
        id: "P-1002",
        name: "캐시미어 니트",
        price: 79_000,
    },
    Item {
        id: "P-1003",
        name: "울 핸드메이드 코트",
        price: 229_000,
    },
    Item {
        id: "P-1004",
        name: "경량 다운 베스트",
        price: 59_000,
    },
    Item {
        id: "P-1005",
        name: "플리스 자켓",
        price: 49_000,
    },
    Item {
        id: "P-1006",
        name: "기모 조거 팬츠",
        price: 29_000,
    },
];

const BRAND_ITEMS: &[Item] = &[
    Item {
        id: "B-2001",
        name: "시그니처 레더 백",
        price: 389_000,
    },
    Item {
        id: "B-2002",
        name: "실크 스카프",
        price: 129_000,
    },
    Item {
        id: "B-2003",
        name: "레더 지갑",
        price: 159_000,
    },
    Item {
        id: "B-2004",
        name: "캐시미어 니트",
        price: 219_000,
    },
];

const SHOP: Profile = Profile {
    base: 4200.0,
    weekend: 1.28,
    ppv: 3.6,
    conv: 0.021,
    aov: 72_000.0,
    channels: [0.20, 0.17, 0.11, 0.08, 0.05, 0.14, 0.06, 0.04, 0.15],
    touch: [0.25, 0.21, 0.13, 0.10, 0.08, 0.14, 0.05, 0.04],
    cpc: [760.0, 640.0, 890.0, 560.0, 430.0],
    paths: SHOP_PATHS,
    items: SHOP_ITEMS,
};

const BRAND: Profile = Profile {
    base: 1250.0,
    weekend: 1.08,
    ppv: 4.4,
    conv: 0.028,
    aov: 168_000.0,
    channels: [0.17, 0.21, 0.10, 0.06, 0.06, 0.13, 0.10, 0.05, 0.12],
    touch: [0.22, 0.26, 0.12, 0.08, 0.09, 0.12, 0.07, 0.04],
    cpc: [1450.0, 1280.0, 1520.0, 1040.0, 840.0],
    paths: BRAND_PATHS,
    items: BRAND_ITEMS,
};

/// 데모 데이터가 있는 프로젝트 (새로 만든 프로젝트는 데이터 없음)
pub fn profile(pid: i32) -> Option<&'static Profile> {
    match pid {
        1 => Some(&SHOP),
        2 => Some(&BRAND),
        _ => None,
    }
}

// ---------------------------------------------------------------- 시계

/// 데모 기준 시각
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub now_ms: i64,
    pub today: Day,
}

impl Clock {
    pub fn new(now_ms: i64) -> Self {
        Self {
            now_ms,
            today: Day::seoul_today(now_ms),
        }
    }
}

/// 서울 자정의 epoch ms
pub fn day_start_ms(d: Day) -> i64 {
    d.0 as i64 * DAY_MS - SEOUL_MS
}

/// 하루 중 `minute`분까지 쌓인 트래픽 비중 (0~1)
pub fn cum_fraction(minute: i64) -> f64 {
    let total: f64 = HOURLY.iter().sum();
    let m = minute.clamp(0, 1440);
    let h = (m / 60) as usize;
    let mut acc: f64 = HOURLY[..h.min(24)].iter().sum();
    if h < 24 {
        acc += HOURLY[h] * (m % 60) as f64 / 60.0;
    }
    acc / total
}

/// 시간대 비중으로 하루 중 분(0..1440) 고르기
fn sample_minute(u: f64) -> i64 {
    let total: f64 = HOURLY.iter().sum();
    let mut acc = 0.0;
    for (h, w) in HOURLY.iter().enumerate() {
        let next = acc + w / total;
        if u < next {
            let within = ((u - acc) / (w / total) * 60.0) as i64;
            return h as i64 * 60 + within.clamp(0, 59);
        }
        acc = next;
    }
    1439
}

/// 프로모션 (블랙프라이데이): 며칠 전 → 배수
fn promo(days_ago: i32) -> f64 {
    match days_ago {
        6 => 1.35,
        7 => 2.4,
        8 => 1.9,
        9 => 1.45,
        _ => 1.0,
    }
}

pub fn is_promo(days_ago: i32) -> bool {
    (6..=9).contains(&days_ago)
}

/// 하루 전체 세션 (오늘도 하루 전체 기준). 데이터 없는 날은 0.
pub fn full_sessions(pid: i32, day: Day, c: Clock) -> f64 {
    let Some(p) = profile(pid) else { return 0.0 };
    let ago = c.today.0 - day.0;
    if !(0..HISTORY_DAYS).contains(&ago) {
        return 0.0;
    }
    let growth = 0.6 + 0.4 * (HISTORY_DAYS - 1 - ago) as f64 / (HISTORY_DAYS - 1) as f64;
    let wd = match day.weekday() {
        0 => 0.96,
        4 => 1.04,
        5 | 6 => p.weekend,
        _ => 1.0,
    };
    let noise = 0.9 + 0.2 * unit(&[pid as i64, day.0 as i64, 1]);
    p.base * growth * wd * promo(ago) * noise
}

/// 그날 현재까지 쌓인 비중 (지난 날 1, 오늘은 시각 비례, 미래 0)
pub fn day_fraction(day: Day, c: Clock) -> f64 {
    if day < c.today {
        1.0
    } else if day > c.today {
        0.0
    } else {
        cum_fraction((c.now_ms - day_start_ms(day)) / MINUTE_MS)
    }
}

// ---------------------------------------------------------------- 하루 트래픽

pub const EVENTS: [&str; 10] = [
    "page_view",
    "scroll",
    "view_item",
    "view_item_list",
    "add_to_cart",
    "begin_checkout",
    "purchase",
    "coupon_download",
    "outbound_click",
    "signup_complete",
];

/// 이벤트별 방문자 비율 (발생 수 대비)
pub fn event_visitor_ratio(name: &str) -> f64 {
    match name {
        "page_view" => 0.24,
        "scroll" => 0.3,
        "view_item" => 0.45,
        "view_item_list" => 0.5,
        "add_to_cart" => 0.72,
        "begin_checkout" => 0.85,
        "purchase" => 0.97,
        "coupon_download" => 0.8,
        "outbound_click" => 0.7,
        _ => 0.98,
    }
}

#[derive(Debug, Clone, Default)]
pub struct DayTraffic {
    pub sessions: u64,
    pub visitors: u64,
    pub pageviews: u64,
    pub channel_sessions: [u64; 9],
    pub channel_pv: [u64; 9],
    pub path_pv: Vec<u64>,
    pub path_entrances: Vec<u64>,
    /// [`EVENTS`] 순서
    pub events: [u64; 10],
}

impl DayTraffic {
    pub fn total_events(&self) -> u64 {
        self.events.iter().sum()
    }
}

pub fn day_traffic(pid: i32, day: Day, c: Clock) -> DayTraffic {
    let Some(p) = profile(pid) else {
        return DayTraffic::default();
    };
    let full = full_sessions(pid, day, c);
    let frac = day_fraction(day, c);
    let sessions = (full * frac).round() as u64;
    if sessions == 0 {
        return DayTraffic {
            path_pv: vec![0; p.paths.len()],
            path_entrances: vec![0; p.paths.len()],
            ..Default::default()
        };
    }
    let (d, id) = (day.0 as i64, pid as i64);
    let ago = c.today.0 - day.0;
    let ppv = p.ppv * (0.93 + 0.14 * unit(&[id, d, 2]));
    let pageviews = (sessions as f64 * ppv).round() as u64;
    let visitors = (sessions as f64 * (0.78 + 0.06 * unit(&[id, d, 3]))).round() as u64;

    let mut cw = p.channels;
    for (i, w) in cw.iter_mut().enumerate() {
        *w *= 0.85 + 0.3 * unit(&[id, d, 10 + i as i64]);
        // 프로모션 기간에는 메타·카카오 광고를 늘린다
        if is_promo(ago) && (i == 1 || i == 3) {
            *w *= 1.6;
        }
    }
    let channel_sessions: [u64; 9] = split(sessions, &cw).try_into().unwrap_or_default();
    // 직접·자연 검색 방문이 조금 더 깊게 본다
    let depth = [0.9, 0.8, 0.95, 0.85, 1.0, 1.15, 0.9, 1.0, 1.3];
    let pvw: Vec<f64> = (0..9)
        .map(|i| channel_sessions[i] as f64 * depth[i])
        .collect();
    let channel_pv: [u64; 9] = split(pageviews, &pvw).try_into().unwrap_or_default();

    let boost = |path: &str| {
        if is_promo(ago) && (path.starts_with("/event/") || path == "/cart") {
            3.0
        } else {
            1.0
        }
    };
    let pw: Vec<f64> = p
        .paths
        .iter()
        .enumerate()
        .map(|(i, x)| x.pv * boost(x.path) * (0.9 + 0.2 * unit(&[id, d, 40 + i as i64])))
        .collect();
    let lw: Vec<f64> = p.paths.iter().map(|x| x.landing * boost(x.path)).collect();
    let path_pv = split(pageviews, &pw);
    let path_entrances = split(sessions, &lw);

    let pvf = pageviews as f64;
    let sf = sessions as f64;
    let view_item = (pvf * 0.30).round();
    let atc = (view_item * (0.15 + 0.03 * unit(&[id, d, 5]))).round();
    let checkout = (atc * 0.5).round();
    let purchases = orders_on(pid, day, c).len() as f64;
    let coupon = (sf * 0.025 * if is_promo(ago) { 3.0 } else { 1.0 }).round();
    let events = [
        pageviews,
        (pvf * 0.45).round() as u64,
        view_item as u64,
        (pvf * 0.22).round() as u64,
        atc as u64,
        (checkout.max(purchases)) as u64,
        purchases as u64,
        coupon as u64,
        (pvf * 0.015).round() as u64,
        (sf * 0.011).round() as u64,
    ];
    DayTraffic {
        sessions,
        visitors,
        pageviews,
        channel_sessions,
        channel_pv,
        path_pv,
        path_entrances,
        events,
    }
}

/// 기간 순 방문자 근사: 날마다 겹치는 방문자를 뺀다 (하루면 그대로).
pub fn uniq(daily_sum: u64, days: usize) -> u64 {
    if days <= 1 {
        return daily_sum;
    }
    (daily_sum as f64 * (0.6 + 0.4 / (days as f64).sqrt())).round() as u64
}

// ---------------------------------------------------------------- 주문

#[derive(Debug, Clone)]
pub struct Order {
    pub id: String,
    pub ts_ms: i64,
    pub gross: i64,
    pub refunded: i64,
    pub status: &'static str,
    pub source: &'static str,
    /// 시간순 (lookback 적용 전, 최대 약 30일 전까지)
    pub touches: Vec<Touch>,
}

impl Order {
    pub fn effective(&self) -> i64 {
        if self.status == "CANCELLED" {
            0
        } else {
            self.gross - self.refunded
        }
    }

    pub fn cancelled(&self) -> bool {
        self.status == "CANCELLED"
    }
}

pub fn order_id(pid: i32, day: Day, i: usize) -> String {
    let (y, m, d) = day.ymd();
    format!("ORD-{:02}{m:02}{d:02}-{pid}{i:04}", y % 100)
}

/// 주문번호 → (날짜, 순번). 형식이 다르면 None.
pub fn parse_order_id(pid: i32, id: &str, c: Clock) -> Option<(Day, usize)> {
    let rest = id.strip_prefix("ORD-")?;
    let (date, tail) = rest.split_once('-')?;
    if date.len() != 6 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let idx = tail.strip_prefix(&pid.to_string())?;
    if idx.len() != 4 || !idx.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let yy: i32 = date[0..2].parse().ok()?;
    let century = c.today.ymd().0 / 100 * 100;
    let day = Day::from_ymd(
        century + yy,
        date[2..4].parse().ok()?,
        date[4..6].parse().ok()?,
    )?;
    Some((day, idx.parse().ok()?))
}

fn round_to(v: f64, unit: i64) -> i64 {
    ((v / unit as f64).round() as i64).max(1) * unit
}

/// 그날의 주문 (지금 시각까지, 시간순)
pub fn orders_on(pid: i32, day: Day, c: Clock) -> Vec<Order> {
    let Some(p) = profile(pid) else {
        return vec![];
    };
    let full = full_sessions(pid, day, c);
    if full <= 0.0 {
        return vec![];
    }
    let ago = c.today.0 - day.0;
    let lift = if is_promo(ago) { 1.25 } else { 1.0 };
    let n = (full * p.conv * lift).round() as usize;
    let start = day_start_ms(day);
    let (id, d) = (pid as i64, day.0 as i64);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let u = |salt: i64| unit(&[id, d, 1000 + i as i64, salt]);
        let ts = start + sample_minute(u(1)) * MINUTE_MS + (u(2) * 59_000.0) as i64;
        if ts > c.now_ms {
            continue;
        }
        let gross = round_to(p.aov * (0.35 + 1.1 * u(4) + 0.9 * u(5).powi(3)), 100);
        let st = u(6);
        let (status, refunded) = if st < 0.04 {
            ("CANCELLED", 0)
        } else if st < 0.07 {
            (
                "PARTIALLY_REFUNDED",
                round_to(gross as f64 * 0.3, 100).min(gross),
            )
        } else {
            ("CONFIRMED", 0)
        };
        out.push(Order {
            id: order_id(pid, day, i),
            ts_ms: ts,
            gross,
            refunded,
            status,
            source: if u(7) < 0.85 { "s2s" } else { "client" },
            touches: touches(p, &[id, d, i as i64], ts),
        });
    }
    out.sort_by(|a, b| a.ts_ms.cmp(&b.ts_ms).then_with(|| a.id.cmp(&b.id)));
    out
}

/// 주문 전 접점. 0~4개, 마지막 접점은 주문 직전, 앞 접점은 며칠씩 앞선다.
fn touches(p: &Profile, key: &[i64], ordered_at: i64) -> Vec<Touch> {
    let u = |salt: i64| {
        let mut k = key.to_vec();
        k.push(5000 + salt);
        unit(&k)
    };
    let k = match u(0) {
        x if x < 0.14 => 0,
        x if x < 0.56 => 1,
        x if x < 0.82 => 2,
        x if x < 0.95 => 3,
        _ => 4,
    };
    let mut ts = ordered_at - (5.0 + u(1) * 235.0) as i64 * MINUTE_MS;
    let mut out = Vec::with_capacity(k);
    for j in 0..k {
        let mut w = p.touch;
        // 리타겟팅(크리테오·메타)은 마지막 접점, 자연 검색은 첫 접점에 많다
        if j == 0 {
            w[4] *= 2.0;
            w[1] *= 1.3;
        } else {
            w[5] *= 1.6;
            w[2] *= 1.3;
        }
        let ci = pick(&w, u(10 + j as i64));
        let ch = CHANNELS[ci];
        let camps = campaigns(ch);
        let camp = camps[pick(&vec![1.0; camps.len()], u(20 + j as i64))];
        out.push(Touch {
            ts_ms: ts,
            channel: ch.to_owned(),
            campaign: camp.to_owned(),
        });
        // 앞 접점까지 간격: 몇 시간 ~ 10일
        let gap_days = 0.05 + u(30 + j as i64).powi(2) * 10.0;
        ts -= (gap_days * DAY_MS as f64) as i64;
    }
    out.reverse();
    out
}

/// 기간 안 주문 (시간순)
pub fn orders_in(pid: i32, from: Day, to: Day, c: Clock) -> Vec<Order> {
    let lo = from.max(c.today.add(-(HISTORY_DAYS - 1)));
    let hi = to.min(c.today);
    let mut out = vec![];
    let mut d = lo;
    while d <= hi {
        out.extend(orders_on(pid, d, c));
        d = d.add(1);
    }
    out
}

// ---------------------------------------------------------------- 실시간

/// 분 구간 시작 ms의 페이지뷰 (그 분 번호로만 정해진다)
pub fn minute_pageviews(pid: i32, minute_start: i64, c: Clock) -> u64 {
    let Some(p) = profile(pid) else { return 0 };
    if minute_start > c.now_ms {
        return 0;
    }
    let day = Day::seoul_today(minute_start);
    let full = full_sessions(pid, day, c);
    let m = (minute_start - day_start_ms(day)) / MINUTE_MS;
    let h = (m / 60).clamp(0, 23) as usize;
    let total: f64 = HOURLY.iter().sum();
    let rate = full * p.ppv * HOURLY[h] / total / 60.0;
    let noise = 0.6 + 0.8 * unit(&[pid as i64, minute_start / MINUTE_MS, 77]);
    (rate * noise).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_keeps_the_total() {
        assert_eq!(split(10, &[1.0, 1.0, 1.0]).iter().sum::<u64>(), 10);
        assert_eq!(split(0, &[1.0, 2.0]), vec![0, 0]);
        assert_eq!(split(5, &[0.0, 0.0]), vec![0, 0]);
        assert_eq!(split(7, &[1.0, 0.0]), vec![7, 0]);
        for t in [1u64, 99, 12_345] {
            assert_eq!(split(t, &[0.3, 0.1, 0.6, 0.0001]).iter().sum::<u64>(), t);
        }
    }

    #[test]
    fn cum_fraction_is_monotonic() {
        assert_eq!(cum_fraction(0), 0.0);
        assert!((cum_fraction(1440) - 1.0).abs() < 1e-12);
        let mut prev = 0.0;
        for m in 0..=1440 {
            let f = cum_fraction(m);
            assert!(f >= prev);
            prev = f;
        }
        for u in [0.0, 0.3, 0.999_999] {
            assert!((0..1440).contains(&sample_minute(u)));
        }
    }

    #[test]
    fn order_ids_round_trip() {
        let c = Clock::new(0);
        let d = Day::from_ymd(1970, 1, 1).unwrap();
        let id = order_id(12, d, 37);
        assert_eq!(id, "ORD-700101-120037");
        assert_eq!(parse_order_id(12, &id, c), Some((d, 37)));
        assert_eq!(parse_order_id(1, &id, c), None);
        for bad in [
            "",
            "ORD-",
            "ORD-700101-12",
            "ORD-7001a1-120037",
            "X-700101-120037",
        ] {
            assert_eq!(parse_order_id(12, bad, c), None, "{bad}");
        }
    }
}

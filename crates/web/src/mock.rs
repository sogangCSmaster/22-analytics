//! 예시 데이터. Phase 1에서 api 응답으로 교체한다.
//! 수치는 설명용이며 실제 데이터가 아니다 (매뉴얼 1.D(2) 예시 기반).

#[derive(Clone, PartialEq)]
pub struct ChannelRevenue {
    pub key: &'static str,
    pub label: &'static str,
    pub raw: i64,
    pub last_click: i64,
    pub first_click: i64,
    pub linear: i64,
    pub time_decay: i64,
    pub assists: u32,
    pub spend: i64,
}

pub fn channel_revenue() -> Vec<ChannelRevenue> {
    vec![
        ChannelRevenue {
            key: "naver_sa",
            label: "네이버 검색광고",
            raw: 72_000_000,
            last_click: 38_000_000,
            first_click: 52_000_000,
            linear: 44_500_000,
            time_decay: 41_200_000,
            assists: 412,
            spend: 12_000_000,
        },
        ChannelRevenue {
            key: "meta",
            label: "메타",
            raw: 64_000_000,
            last_click: 46_000_000,
            first_click: 30_000_000,
            linear: 38_000_000,
            time_decay: 40_800_000,
            assists: 198,
            spend: 14_000_000,
        },
        ChannelRevenue {
            key: "google_ads",
            label: "구글 애즈",
            raw: 34_000_000,
            last_click: 21_000_000,
            first_click: 22_000_000,
            linear: 21_500_000,
            time_decay: 21_300_000,
            assists: 120,
            spend: 9_000_000,
        },
        ChannelRevenue {
            key: "kakao",
            label: "카카오",
            raw: 18_000_000,
            last_click: 11_000_000,
            first_click: 9_000_000,
            linear: 10_000_000,
            time_decay: 10_400_000,
            assists: 64,
            spend: 5_000_000,
        },
        ChannelRevenue {
            key: "organic_search",
            label: "자연 검색",
            raw: 12_000_000,
            last_click: 14_000_000,
            first_click: 17_000_000,
            linear: 16_000_000,
            time_decay: 16_300_000,
            assists: 233,
            spend: 0,
        },
    ]
}

/// 중복 매체 조합 (주문 수 기준)
pub fn overlap_pairs() -> Vec<(&'static str, i64)> {
    vec![
        ("네이버 검색광고 → 메타", 1_284),
        ("메타 → 네이버 검색광고", 612),
        ("네이버 검색광고 → 구글 애즈", 341),
        ("구글 애즈 → 메타", 220),
        ("카카오 → 네이버 검색광고", 145),
    ]
}

pub struct DailyPoint {
    pub label: &'static str,
    pub visitors: i64,
}

pub fn daily_visitors() -> Vec<DailyPoint> {
    let v = [
        ("11/08", 8_210),
        ("11/09", 7_980),
        ("11/10", 9_420),
        ("11/11", 12_830),
        ("11/12", 10_210),
        ("11/13", 9_870),
        ("11/14", 9_120),
        ("11/15", 8_740),
        ("11/16", 8_460),
        ("11/17", 10_330),
        ("11/18", 11_020),
        ("11/19", 10_860),
        ("11/20", 11_940),
        ("11/21", 12_410),
    ];
    v.into_iter()
        .map(|(label, visitors)| DailyPoint { label, visitors })
        .collect()
}

pub fn top_channels() -> Vec<(&'static str, i64)> {
    vec![
        ("네이버 검색광고", 38_420),
        ("메타", 27_310),
        ("자연 검색", 21_880),
        ("구글 애즈", 12_940),
        ("직접 유입", 9_870),
        ("카카오", 6_210),
    ]
}

pub struct PageRow {
    pub path: &'static str,
    pub pageviews: i64,
    pub visitors: i64,
    pub bounce: f64,
}

pub fn top_pages() -> Vec<PageRow> {
    vec![
        PageRow {
            path: "/",
            pageviews: 48_210,
            visitors: 31_200,
            bounce: 0.41,
        },
        PageRow {
            path: "/goods/winter-padding",
            pageviews: 21_430,
            visitors: 15_980,
            bounce: 0.33,
        },
        PageRow {
            path: "/event/black-friday",
            pageviews: 18_920,
            visitors: 14_210,
            bounce: 0.52,
        },
        PageRow {
            path: "/cart",
            pageviews: 9_840,
            visitors: 6_420,
            bounce: 0.12,
        },
        PageRow {
            path: "/order/complete",
            pageviews: 2_315,
            visitors: 2_210,
            bounce: 0.05,
        },
    ]
}

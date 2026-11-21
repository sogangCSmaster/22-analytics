//! 주문 1건의 기여 결과 계산과 기존 결과와의 비교 (I/O 없음).

use analytics_ch::attribution::{AttributedRow, OrderForAttribution, TouchRow};
use analytics_core::attribution::{
    DAY_MS, HOUR_MS, Model, Params, Touch, attribute, within_lookback,
};
use analytics_core::channel::{
    ChannelInput, ChannelRule, ORGANIC_SEARCH, ORGANIC_SOCIAL, REFERRAL, classify,
    is_touchpoint_channel,
};
use analytics_core::orders::OrderStatus;
use analytics_pg::attribution_settings::AttributionSettings;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// 기여 대상 채널인지 (direct·내부 이동은 항상 제외, 자연 유입·추천은 설정에 따름).
pub fn is_eligible(channel: &str, settings: &AttributionSettings) -> bool {
    if !is_touchpoint_channel(channel) {
        return false;
    }
    match channel {
        ORGANIC_SEARCH | ORGANIC_SOCIAL => settings.include_organic,
        REFERRAL => settings.include_referral,
        _ => true,
    }
}

/// 터치포인트 원본을 현재 채널 규칙으로 다시 분류한다. 기여 대상이 아니면 `None`.
pub fn to_touch(
    row: &TouchRow,
    rules: &[ChannelRule],
    settings: &AttributionSettings,
) -> Option<Touch> {
    let channel = classify(
        &ChannelInput {
            utm_source: &row.utm_source,
            utm_medium: &row.utm_medium,
            utm_campaign: &row.utm_campaign,
            click_id_type: &row.click_id_type,
            referrer_host: &row.referrer_host,
            page_host: &row.page_host,
        },
        rules,
    );
    is_eligible(&channel, settings).then(|| Touch {
        ts_ms: row.ts_ms,
        channel,
        campaign: row.utm_campaign.clone(),
    })
}

/// 주문 식별자 집합(`ids`)에 속한 터치포인트만 골라 분류한다.
/// 같은 (visitor_id, session_id, ts)는 재전송 중복으로 보고 하나만 남긴다.
pub fn select_touches(
    ids: &[Uuid],
    rows: &[TouchRow],
    rules: &[ChannelRule],
    settings: &AttributionSettings,
) -> Vec<Touch> {
    let ids: HashSet<&Uuid> = ids.iter().collect();
    let mut seen = HashSet::new();
    rows.iter()
        .filter(|r| ids.contains(&r.visitor_id))
        .filter(|r| seen.insert((r.visitor_id, r.session_id, r.ts_ms)))
        .filter_map(|r| to_touch(r, rules, settings))
        .collect()
}

/// 주문이 기여 대상인지 (취소·0원 이하는 기존 결과를 삭제 표시만 한다).
pub fn is_live(order: &OrderForAttribution) -> bool {
    order.status != OrderStatus::Cancelled && order.effective_amount > 0
}

/// 원하는 결과 행 (lookback × 모델). `computed_at`은 0으로 두고 [`plan_writes`]가 채운다.
/// `touches`는 이 주문 식별자들의 접점 전체(정렬·기간 필터 전)다.
/// `ordered_at`은 주문 행 값을 그대로 쓴다 (파티션 키라서 다시 계산하면 안 된다).
pub fn desired_rows(
    project_id: u32,
    order: &OrderForAttribution,
    touches: &[Touch],
    settings: &AttributionSettings,
) -> Vec<AttributedRow> {
    if !is_live(order) {
        return Vec::new();
    }
    // 같은 ms 접점도 입력 순서와 무관하게 같은 결과가 나오도록 전체 순서를 고정한다.
    let mut sorted = touches.to_vec();
    sorted.sort_by(|a, b| {
        (a.ts_ms, &a.channel, &a.campaign).cmp(&(b.ts_ms, &b.channel, &b.campaign))
    });
    let half_life_ms = i64::from(settings.half_life_hours) * HOUR_MS;
    let mut out = Vec::new();
    for &lb in &settings.lookback_days {
        let params = Params {
            lookback_ms: i64::from(lb) * DAY_MS,
            half_life_ms,
        };
        let window = within_lookback(sorted.clone(), order.ordered_at_ms, params.lookback_ms);
        for model in Model::ALL {
            let shares = attribute(
                model,
                order.effective_amount,
                order.ordered_at_ms,
                &window,
                &params,
            );
            out.extend(shares.into_iter().map(|s| AttributedRow {
                project_id,
                model: model.as_str().to_owned(),
                lookback_days: lb,
                order_id: order.order_id.clone(),
                touch_index: s.touch_index,
                channel: s.channel,
                utm_campaign: s.campaign,
                touch_ts: s.touch_ts_ms,
                ordered_at: order.ordered_at_ms,
                amount: s.amount,
                is_assist: u8::from(s.is_assist),
                computed_at: 0,
                is_deleted: 0,
            }));
        }
    }
    out
}

/// (model, lookback_days, touch_index, ordered_at): 한 주문 안의 행 키.
type Key = (String, u8, u16, i64);
/// 비교 대상 내용: (channel, utm_campaign, touch_ts, amount, is_assist)
type Content = (String, String, Option<i64>, i64, u8);

/// 기존 결과와 비교해 써야 할 행을 만든다. 바뀐 게 없으면 빈 벡터.
///
/// - 바뀌었으면 원하는 행 전부를 새 `computed_at`으로 쓴다.
/// - 기존에 살아 있던 키가 더는 없으면 같은 키(같은 ordered_at 파티션)로 `is_deleted=1`을 쓴다.
/// - `computed_at = max(now_ms, 기존 최대 computed_at + 1)`: ReplacingMergeTree가 반드시 새 행을 고른다.
pub fn plan_writes(
    desired: Vec<AttributedRow>,
    existing: &[AttributedRow],
    now_ms: i64,
) -> Vec<AttributedRow> {
    let alive: HashMap<Key, Content> = existing
        .iter()
        .filter(|e| e.is_deleted == 0)
        .map(|e| {
            (
                (
                    e.model.clone(),
                    e.lookback_days,
                    e.touch_index,
                    e.ordered_at,
                ),
                (
                    e.channel.clone(),
                    e.utm_campaign.clone(),
                    e.touch_ts,
                    e.amount,
                    e.is_assist,
                ),
            )
        })
        .collect();
    let wanted: HashMap<Key, Content> = desired
        .iter()
        .map(|r| {
            (
                (
                    r.model.clone(),
                    r.lookback_days,
                    r.touch_index,
                    r.ordered_at,
                ),
                (
                    r.channel.clone(),
                    r.utm_campaign.clone(),
                    r.touch_ts,
                    r.amount,
                    r.is_assist,
                ),
            )
        })
        .collect();
    if alive == wanted {
        return Vec::new();
    }
    let computed_at = existing
        .iter()
        .map(|e| e.computed_at.saturating_add(1))
        .fold(now_ms, i64::max);

    let mut out: Vec<AttributedRow> = desired
        .into_iter()
        .map(|mut r| {
            r.computed_at = computed_at;
            r.is_deleted = 0;
            r
        })
        .collect();
    out.extend(
        existing
            .iter()
            .filter(|e| e.is_deleted == 0)
            .filter(|e| {
                !wanted.contains_key(&(
                    e.model.clone(),
                    e.lookback_days,
                    e.touch_index,
                    e.ordered_at,
                ))
            })
            .map(|e| AttributedRow {
                computed_at,
                is_deleted: 1,
                ..e.clone()
            }),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use analytics_core::channel::RuleMatch;

    const P: u32 = 7;
    const MIN: i64 = 60_000;
    /// 2026-11-20 18:45 KST (= 09:45 UTC)
    const ORDER_AT: i64 = 1_795_167_900_000;

    fn order(amount: i64) -> OrderForAttribution {
        OrderForAttribution {
            order_id: "ORD-9901".into(),
            status: OrderStatus::Confirmed,
            visitor_id: Some(Uuid::nil()),
            member_hash: String::new(),
            effective_amount: amount,
            ordered_at_ms: ORDER_AT,
            version: 1,
        }
    }

    fn touch(ts: i64, ch: &str) -> Touch {
        Touch {
            ts_ms: ts,
            channel: ch.into(),
            campaign: format!("{ch}_camp"),
        }
    }

    fn trow(visitor: Uuid, ts: i64, src: &str, med: &str, refh: &str) -> TouchRow {
        TouchRow {
            visitor_id: visitor,
            session_id: Uuid::from_u128(ts as u128),
            ts_ms: ts,
            utm_source: src.into(),
            utm_medium: med.into(),
            utm_campaign: "c".into(),
            click_id_type: String::new(),
            referrer_host: refh.into(),
            page_host: "shop.example.com".into(),
        }
    }

    fn manual_touches() -> Vec<Touch> {
        vec![
            touch(ORDER_AT - 5 * MIN, "meta"),
            touch(ORDER_AT - 35 * MIN, "naver_sa"),
        ]
    }

    fn pick<'a>(rows: &'a [AttributedRow], model: &str, lb: u8) -> Vec<(&'a str, i64)> {
        rows.iter()
            .filter(|r| r.model == model && r.lookback_days == lb)
            .map(|r| (r.channel.as_str(), r.amount))
            .collect()
    }

    fn existing_from(rows: &[AttributedRow], computed_at: i64) -> Vec<AttributedRow> {
        rows.iter()
            .map(|r| AttributedRow {
                computed_at,
                ..r.clone()
            })
            .collect()
    }

    #[test]
    fn eligibility_follows_settings() {
        let all = AttributionSettings::default();
        for ch in ["direct", "(internal)"] {
            assert!(!is_eligible(ch, &all), "{ch}");
        }
        for ch in [
            "meta",
            "naver_sa",
            "organic_search",
            "organic_social",
            "referral",
        ] {
            assert!(is_eligible(ch, &all), "{ch}");
        }
        let paid_only = AttributionSettings {
            include_organic: false,
            include_referral: false,
            ..Default::default()
        };
        for ch in ["organic_search", "organic_social", "referral"] {
            assert!(!is_eligible(ch, &paid_only), "{ch}");
        }
        assert!(is_eligible("meta", &paid_only));
        let no_ref = AttributionSettings {
            include_referral: false,
            ..Default::default()
        };
        assert!(is_eligible("organic_search", &no_ref));
        assert!(!is_eligible("referral", &no_ref));
    }

    #[test]
    fn to_touch_recomputes_channel_with_current_rules() {
        let s = AttributionSettings::default();
        let r = trow(Uuid::nil(), 5, "NAVER", "cpc", "");
        let t = to_touch(&r, &[], &s).unwrap();
        assert_eq!(
            (t.channel.as_str(), t.ts_ms, t.campaign.as_str()),
            ("naver_sa", 5, "c")
        );

        let rules = vec![ChannelRule {
            channel: "naver_brand".into(),
            when: RuleMatch {
                utm_source: vec!["naver".into()],
                ..Default::default()
            },
        }];
        assert_eq!(to_touch(&r, &rules, &s).unwrap().channel, "naver_brand");

        // direct / 내부 이동은 터치포인트가 아니다
        assert!(to_touch(&trow(Uuid::nil(), 1, "", "", ""), &[], &s).is_none());
        assert!(to_touch(&trow(Uuid::nil(), 1, "", "", "shop.example.com"), &[], &s).is_none());
        let paid_only = AttributionSettings {
            include_organic: false,
            include_referral: false,
            ..Default::default()
        };
        assert!(
            to_touch(
                &trow(Uuid::nil(), 1, "", "", "www.google.com"),
                &[],
                &paid_only
            )
            .is_none()
        );
        assert!(
            to_touch(
                &trow(Uuid::nil(), 1, "", "", "news.example.org"),
                &[],
                &paid_only
            )
            .is_none()
        );
    }

    #[test]
    fn select_touches_filters_visitors_and_dedups_retries() {
        let s = AttributionSettings::default();
        let (v1, v2, other) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        let dup = trow(v1, 10, "naver", "cpc", "");
        let rows = vec![
            dup.clone(),
            dup,
            trow(v2, 10, "meta", "", ""),
            trow(other, 11, "kakao", "", ""),
        ];
        let got = select_touches(&[v1, v2], &rows, &[], &s);
        let mut ch: Vec<&str> = got.iter().map(|t| t.channel.as_str()).collect();
        ch.sort();
        assert_eq!(ch, vec!["meta", "naver_sa"]);
        assert!(select_touches(&[], &rows, &[], &s).is_empty());
    }

    #[test]
    fn manual_scenario_rows() {
        let s = AttributionSettings::default();
        let rows = desired_rows(P, &order(100_000), &manual_touches(), &s);
        // lookback 2개 × 모델 5개 × 접점 2개
        assert_eq!(rows.len(), 20);
        for lb in [7, 30] {
            assert_eq!(
                pick(&rows, "last_click", lb),
                vec![("naver_sa", 0), ("meta", 100_000)]
            );
            assert_eq!(
                pick(&rows, "first_click", lb),
                vec![("naver_sa", 100_000), ("meta", 0)]
            );
            assert_eq!(
                pick(&rows, "linear", lb),
                vec![("naver_sa", 50_000), ("meta", 50_000)]
            );
            assert_eq!(
                pick(&rows, "raw_sum", lb),
                vec![("naver_sa", 100_000), ("meta", 100_000)]
            );
            let td: i64 = pick(&rows, "time_decay", lb).iter().map(|x| x.1).sum();
            assert_eq!(td, 100_000);
        }
        let assist: Vec<(u16, u8)> = rows
            .iter()
            .filter(|r| r.model == "last_click" && r.lookback_days == 7)
            .map(|r| (r.touch_index, r.is_assist))
            .collect();
        assert_eq!(assist, vec![(1, 1), (2, 0)]);
        let r = &rows[0];
        assert_eq!(
            (r.project_id, r.order_id.as_str(), r.ordered_at),
            (P, "ORD-9901", ORDER_AT)
        );
        assert_eq!(r.touch_ts, Some(ORDER_AT - 35 * MIN));
        assert_eq!(r.utm_campaign, "naver_sa_camp");
        assert!(rows.iter().all(|r| r.computed_at == 0 && r.is_deleted == 0));
    }

    #[test]
    fn lookbacks_are_applied_per_row_set() {
        let s = AttributionSettings::default();
        // 10일 전 접점은 30일에만, 미래 접점은 어디에도 없다
        let touches = vec![
            touch(ORDER_AT - 10 * DAY_MS, "kakao"),
            touch(ORDER_AT - 7 * DAY_MS, "naver_sa"),
            touch(ORDER_AT + 1, "future"),
        ];
        let rows = desired_rows(P, &order(1_000), &touches, &s);
        assert_eq!(pick(&rows, "last_click", 7), vec![("naver_sa", 1_000)]);
        assert_eq!(
            pick(&rows, "first_click", 30),
            vec![("kakao", 1_000), ("naver_sa", 0)]
        );
    }

    #[test]
    fn no_touches_is_unattributed() {
        let s = AttributionSettings::default();
        let rows = desired_rows(P, &order(5_000), &[], &s);
        assert_eq!(rows.len(), 10);
        assert!(rows.iter().all(|r| r.touch_index == 0
            && r.channel == "(unattributed)"
            && r.touch_ts.is_none()
            && r.amount == 5_000));
    }

    #[test]
    fn dead_orders_have_no_rows() {
        let s = AttributionSettings::default();
        let mut o = order(100_000);
        assert!(is_live(&o));
        o.status = OrderStatus::Cancelled;
        assert!(!is_live(&o));
        assert!(desired_rows(P, &o, &manual_touches(), &s).is_empty());
        let zero = order(0);
        assert!(!is_live(&zero));
        assert!(desired_rows(P, &zero, &manual_touches(), &s).is_empty());
        assert!(!is_live(&order(-5)));
        let mut partial = order(30_000);
        partial.status = OrderStatus::PartiallyRefunded;
        assert!(is_live(&partial));
    }

    #[test]
    fn plan_writes_new_unchanged_changed() {
        let s = AttributionSettings::default();
        let desired = desired_rows(P, &order(100_000), &manual_touches(), &s);

        // 처음: 전부 쓴다
        let w = plan_writes(desired.clone(), &[], 1_000);
        assert_eq!(w.len(), 20);
        assert!(
            w.iter()
                .all(|r| r.computed_at == 1_000 && r.is_deleted == 0)
        );

        // 같은 결과: 아무것도 쓰지 않는다
        let existing = existing_from(&w, 1_000);
        assert!(plan_writes(desired.clone(), &existing, 2_000).is_empty());

        // 시계가 뒤로 가도 computed_at은 기존보다 커야 한다
        let refunded = desired_rows(P, &order(70_000), &manual_touches(), &s);
        let w2 = plan_writes(refunded, &existing, 500);
        assert_eq!(w2.len(), 20);
        assert!(w2.iter().all(|r| r.computed_at == 1_001));
        assert_eq!(
            pick(&w2, "linear", 7),
            vec![("naver_sa", 35_000), ("meta", 35_000)]
        );
    }

    #[test]
    fn plan_writes_deletes_vanished_keys() {
        let s = AttributionSettings::default();
        let before = plan_writes(desired_rows(P, &order(1_000), &[], &s), &[], 10);
        let existing = existing_from(&before, 10);
        // 늦게 도착한 접점: touch_index 0(미귀속) 행은 삭제 표시
        let after = desired_rows(P, &order(1_000), &[touch(ORDER_AT - MIN, "meta")], &s);
        let w = plan_writes(after, &existing, 20);
        let deleted: Vec<AttributedRow> = w.iter().filter(|r| r.is_deleted == 1).cloned().collect();
        assert_eq!(deleted.len(), 10);
        assert!(
            deleted
                .iter()
                .all(|r| r.touch_index == 0 && r.computed_at == 20)
        );
        assert_eq!(w.iter().filter(|r| r.is_deleted == 0).count(), 10);

        // 취소: 살아 있는 행 전부 삭제 표시, 이미 삭제된 행은 다시 쓰지 않는다
        let state = existing_from(&w, 20);
        let cancel = plan_writes(Vec::new(), &state, 30);
        assert_eq!(cancel.len(), 10);
        assert!(
            cancel
                .iter()
                .all(|r| r.is_deleted == 1 && r.touch_index == 1 && r.computed_at == 30)
        );
        // 취소를 다시 처리해도 쓸 게 없다
        let mut after_cancel = existing_from(&cancel, 30);
        after_cancel.extend(existing_from(&deleted, 20));
        assert!(plan_writes(Vec::new(), &after_cancel, 40).is_empty());
    }

    #[test]
    fn plan_writes_moves_partition_when_ordered_at_changes() {
        let s = AttributionSettings::default();
        let before = plan_writes(desired_rows(P, &order(1_000), &[], &s), &[], 10);
        let existing = existing_from(&before, 10);
        let mut moved = order(1_000);
        moved.ordered_at_ms = ORDER_AT - 40 * DAY_MS;
        let w = plan_writes(desired_rows(P, &moved, &[], &s), &existing, 20);
        // 옛 ordered_at으로 삭제 표시 10개 + 새 ordered_at으로 10개
        assert_eq!(w.len(), 20);
        assert!(
            w.iter()
                .filter(|r| r.is_deleted == 1)
                .all(|r| r.ordered_at == ORDER_AT)
        );
        assert!(
            w.iter()
                .filter(|r| r.is_deleted == 0)
                .all(|r| r.ordered_at == ORDER_AT - 40 * DAY_MS)
        );
    }

    #[test]
    fn same_ms_touches_are_deterministic() {
        let s = AttributionSettings::default();
        let a = vec![
            touch(ORDER_AT - MIN, "naver_sa"),
            touch(ORDER_AT - MIN, "meta"),
        ];
        let b = vec![
            touch(ORDER_AT - MIN, "meta"),
            touch(ORDER_AT - MIN, "naver_sa"),
        ];
        let ra = desired_rows(P, &order(100_001), &a, &s);
        let rb = desired_rows(P, &order(100_001), &b, &s);
        assert_eq!(ra, rb);
        assert_eq!(
            pick(&ra, "linear", 7),
            vec![("meta", 50_000), ("naver_sa", 50_001)]
        );
    }

    #[test]
    fn thousand_touches_conserve() {
        let s = AttributionSettings::default();
        let chans = ["meta", "naver_sa", "kakao"];
        let touches: Vec<Touch> = (0..1_000)
            .map(|i| touch(ORDER_AT - (i + 1) * MIN, chans[(i % 3) as usize]))
            .collect();
        let rows = desired_rows(P, &order(999_999), &touches, &s);
        assert_eq!(rows.len(), 2 * 5 * 1_000);
        for lb in [7u8, 30] {
            for m in ["last_click", "first_click", "linear", "time_decay"] {
                let sum: i64 = pick(&rows, m, lb).iter().map(|x| x.1).sum();
                assert_eq!(sum, 999_999, "{m} {lb}");
            }
            let raw: i64 = pick(&rows, "raw_sum", lb).iter().map(|x| x.1).sum();
            assert_eq!(raw, 3 * 999_999);
        }
        assert_eq!(rows.iter().map(|r| r.touch_index).max(), Some(1_000));
    }
}

//! 중복 소거와 기여 모델 (매뉴얼 2.E).
//!
//! 주문 1건(`order_id`)의 확정 금액을 lookback 기간 안의 터치포인트에 배분한다.
//! **매출 보존 법칙**: `raw_sum`을 제외한 모든 모델에서 배분 합계는 주문 금액과 정확히 같다.
//! 금액은 통화 최소단위 정수(KRW = 원)이고, 나눠떨어지지 않는 나머지는 마지막 접점에 준다.

use crate::channel::UNATTRIBUTED;
use serde::{Deserialize, Serialize};

pub const HOUR_MS: i64 = 60 * 60 * 1000;
pub const DAY_MS: i64 = 24 * HOUR_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Model {
    LastClick,
    FirstClick,
    Linear,
    TimeDecay,
    /// 각 매체가 자기 성과로 주장하는 매출의 근사: 서로 다른 채널마다 주문 금액 전액.
    RawSum,
}

impl Model {
    pub const ALL: [Model; 5] = [
        Model::LastClick,
        Model::FirstClick,
        Model::Linear,
        Model::TimeDecay,
        Model::RawSum,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Model::LastClick => "last_click",
            Model::FirstClick => "first_click",
            Model::Linear => "linear",
            Model::TimeDecay => "time_decay",
            Model::RawSum => "raw_sum",
        }
    }

    /// 매출 보존 법칙 적용 대상인지.
    pub fn conserves_revenue(self) -> bool {
        self != Model::RawSum
    }
}

/// 기여 대상 접점 (터치포인트).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Touch {
    pub ts_ms: i64,
    pub channel: String,
    pub campaign: String,
}

/// 배분 결과 한 줄.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    /// 1부터 시작하는 접점 순번 (시간순). 0은 미귀속.
    pub touch_index: u16,
    pub channel: String,
    pub campaign: String,
    pub touch_ts_ms: Option<i64>,
    pub amount: i64,
    /// last_click에서 0원이 된 접점 (어시스트)
    pub is_assist: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub lookback_ms: i64,
    pub half_life_ms: i64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            lookback_ms: 7 * DAY_MS,
            half_life_ms: 7 * DAY_MS,
        }
    }
}

/// `[ordered_at − lookback, ordered_at]` 안의 접점만 남기고 시간순으로 정렬한다.
pub fn within_lookback(
    mut touches: Vec<Touch>,
    ordered_at_ms: i64,
    lookback_ms: i64,
) -> Vec<Touch> {
    let from = ordered_at_ms - lookback_ms;
    touches.retain(|t| t.ts_ms >= from && t.ts_ms <= ordered_at_ms);
    touches.sort_by(|a, b| {
        a.ts_ms
            .cmp(&b.ts_ms)
            .then_with(|| a.channel.cmp(&b.channel))
    });
    touches
}

/// 주문 1건을 모델에 따라 배분한다. `touches`는 [`within_lookback`]을 거친 값이어야 한다.
pub fn attribute(
    model: Model,
    amount: i64,
    ordered_at_ms: i64,
    touches: &[Touch],
    p: &Params,
) -> Vec<Share> {
    let amount = amount.max(0);
    if touches.is_empty() {
        return vec![Share {
            touch_index: 0,
            channel: UNATTRIBUTED.to_owned(),
            campaign: String::new(),
            touch_ts_ms: None,
            amount,
            is_assist: false,
        }];
    }
    let n = touches.len();
    let mut out: Vec<Share> = touches
        .iter()
        .enumerate()
        .map(|(i, t)| Share {
            touch_index: (i + 1).min(u16::MAX as usize) as u16,
            channel: t.channel.clone(),
            campaign: t.campaign.clone(),
            touch_ts_ms: Some(t.ts_ms),
            amount: 0,
            is_assist: false,
        })
        .collect();

    match model {
        Model::LastClick => {
            out[n - 1].amount = amount;
            for s in &mut out[..n - 1] {
                s.is_assist = true;
            }
        }
        Model::FirstClick => out[0].amount = amount,
        Model::Linear => distribute(&mut out, amount, &vec![1.0; n]),
        Model::TimeDecay => {
            let hl = p.half_life_ms.max(1) as f64;
            let weights: Vec<f64> = touches
                .iter()
                .map(|t| (-((ordered_at_ms - t.ts_ms).max(0) as f64) / hl).exp2())
                .collect();
            distribute(&mut out, amount, &weights);
        }
        Model::RawSum => {
            // 같은 채널은 한 번만: 매체 대시보드는 주문 1건을 매체당 한 번 센다.
            let mut seen = std::collections::HashSet::new();
            for s in out.iter_mut().rev() {
                if seen.insert(s.channel.clone()) {
                    s.amount = amount;
                }
            }
        }
    }
    out
}

/// 가중치 비례로 정수 배분하고, 나머지는 마지막 접점에 더한다.
fn distribute(out: &mut [Share], amount: i64, weights: &[f64]) {
    let total: f64 = weights.iter().sum();
    let mut assigned = 0i64;
    if total > 0.0 && total.is_finite() {
        for (s, w) in out.iter_mut().zip(weights) {
            let a = ((amount as f64) * (w / total)).floor() as i64;
            let a = a.clamp(0, amount - assigned);
            s.amount = a;
            assigned += a;
        }
    }
    if let Some(last) = out.last_mut() {
        last.amount += amount - assigned;
    }
}

/// 매출 보존 법칙 검증.
pub fn conserves(model: Model, amount: i64, shares: &[Share]) -> bool {
    !model.conserves_revenue() || shares.iter().map(|s| s.amount).sum::<i64>() == amount.max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn t(ts: i64, ch: &str) -> Touch {
        Touch {
            ts_ms: ts,
            channel: ch.into(),
            campaign: String::new(),
        }
    }

    /// 매뉴얼 1.A 시나리오: 18:10 네이버, 18:40 메타, 18:45 100,000원 결제 (ORD-9901).
    fn ord_9901() -> (i64, Vec<Touch>) {
        let order = 18 * 60 * MIN + 45 * MIN;
        (
            order,
            vec![
                t(18 * 60 * MIN + 40 * MIN, "meta"),
                t(18 * 60 * MIN + 10 * MIN, "naver"),
            ],
        )
    }

    fn amounts(s: &[Share]) -> Vec<(&str, i64)> {
        s.iter().map(|x| (x.channel.as_str(), x.amount)).collect()
    }

    #[test]
    fn manual_scenario() {
        let (order, raw) = ord_9901();
        let touches = within_lookback(raw, order, 7 * DAY_MS);
        let p = Params::default();

        let last = attribute(Model::LastClick, 100_000, order, &touches, &p);
        assert_eq!(amounts(&last), vec![("naver", 0), ("meta", 100_000)]);
        assert!(last[0].is_assist && !last[1].is_assist);

        let first = attribute(Model::FirstClick, 100_000, order, &touches, &p);
        assert_eq!(amounts(&first), vec![("naver", 100_000), ("meta", 0)]);

        let linear = attribute(Model::Linear, 100_000, order, &touches, &p);
        assert_eq!(amounts(&linear), vec![("naver", 50_000), ("meta", 50_000)]);

        let decay = attribute(Model::TimeDecay, 100_000, order, &touches, &p);
        assert!(decay[1].amount > decay[0].amount);
        assert_eq!(decay.iter().map(|s| s.amount).sum::<i64>(), 100_000);

        // 매체 합산: 네이버 10만 + 메타 10만 = 20만 (보고된 매출 2배)
        let raw = attribute(Model::RawSum, 100_000, order, &touches, &p);
        assert_eq!(raw.iter().map(|s| s.amount).sum::<i64>(), 200_000);
    }

    #[test]
    fn time_decay_half_life() {
        // 반감기 1일: 1일 전 접점 가중치 0.5, 당일 1.0 → 1/3, 2/3
        let order = 10 * DAY_MS;
        let touches = vec![t(order - DAY_MS, "a"), t(order, "b")];
        let p = Params {
            lookback_ms: 7 * DAY_MS,
            half_life_ms: DAY_MS,
        };
        let s = attribute(Model::TimeDecay, 90_000, order, &touches, &p);
        assert_eq!(amounts(&s), vec![("a", 30_000), ("b", 60_000)]);
    }

    #[test]
    fn remainder_goes_to_last_touch() {
        let touches = vec![t(1, "a"), t(2, "b"), t(3, "c")];
        let s = attribute(Model::Linear, 100_000, 10, &touches, &Params::default());
        assert_eq!(
            amounts(&s),
            vec![("a", 33_333), ("b", 33_333), ("c", 33_334)]
        );
    }

    #[test]
    fn no_touches_is_unattributed() {
        let s = attribute(Model::LastClick, 5_000, 10, &[], &Params::default());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].touch_index, 0);
        assert_eq!(s[0].channel, UNATTRIBUTED);
        assert_eq!(s[0].amount, 5_000);
    }

    #[test]
    fn lookback_filters_and_sorts() {
        let order = 30 * DAY_MS;
        let touches = vec![
            t(order + 1, "future"),
            t(order - 8 * DAY_MS, "old"),
            t(order - DAY_MS, "b"),
            t(order - 2 * DAY_MS, "a"),
        ];
        let kept = within_lookback(touches, order, 7 * DAY_MS);
        assert_eq!(
            kept.iter().map(|t| t.channel.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn raw_sum_counts_each_channel_once() {
        let touches = vec![t(1, "naver"), t(2, "meta"), t(3, "naver")];
        let s = attribute(Model::RawSum, 1_000, 10, &touches, &Params::default());
        assert_eq!(s.iter().map(|x| x.amount).sum::<i64>(), 2_000);
        // 같은 채널은 가장 최근 접점에만 기록
        assert_eq!(
            amounts(&s),
            vec![("naver", 0), ("meta", 1_000), ("naver", 1_000)]
        );
    }

    #[test]
    fn negative_amount_is_clamped() {
        let s = attribute(Model::Linear, -10, 10, &[t(1, "a")], &Params::default());
        assert_eq!(s[0].amount, 0);
    }

    /// 무작위 입력으로 매출 보존 법칙을 검증한다 (외부 의존성 없는 LCG).
    #[test]
    fn conservation_law_holds_for_random_inputs() {
        let mut seed: u64 = 0x2222_2026;
        let mut rnd = move |m: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % m
        };
        let channels = ["naver_sa", "meta", "google_ads", "kakao", "organic_search"];
        for _ in 0..5_000 {
            let order = 40 * DAY_MS;
            let n = rnd(12) as usize;
            let touches: Vec<Touch> = (0..n)
                .map(|_| {
                    t(
                        order - rnd(30 * DAY_MS as u64) as i64,
                        channels[rnd(5) as usize],
                    )
                })
                .collect();
            let amount = rnd(10_000_000) as i64;
            let p = Params {
                lookback_ms: (1 + rnd(30)) as i64 * DAY_MS,
                half_life_ms: (1 + rnd(14 * 24)) as i64 * HOUR_MS,
            };
            let touches = within_lookback(touches, order, p.lookback_ms);
            for model in Model::ALL {
                let s = attribute(model, amount, order, &touches, &p);
                assert!(
                    conserves(model, amount, &s),
                    "{model:?} amount={amount} n={}",
                    touches.len()
                );
                assert!(s.iter().all(|x| x.amount >= 0));
            }
        }
    }
}

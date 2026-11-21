//! 이전 기간 비교: 현재·이전 행을 키로 맞추고, 증감률(%)·퍼센트포인트(%p) 변화를 정한다.
//!
//! 비교를 끄면 화면은 이 모듈을 부르지 않는다 (이전 값·증감 표시가 하나도 없다).

use std::cmp::Ordering;
use std::collections::HashMap;

/// 이전 기간 대비 변화.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Change {
    /// 증감률 (0.12 = +12%)
    Pct(f64),
    /// 비율의 차이, 퍼센트포인트 (0.032 = +3.2%p)
    Pp(f64),
    /// 이전 기간에는 0이었다 (∞% 대신 "신규")
    New,
    /// 비교할 수 없다 (둘 다 0, 이전 값을 모름)
    Unknown,
}

/// 값의 증감률. `prev`가 None이면 이전 값을 모른다.
pub fn change(cur: f64, prev: Option<f64>) -> Change {
    let _ = (cur, prev);
    Change::Unknown
}

/// 비율(전환율·중복률·ROAS)의 퍼센트포인트 변화.
pub fn pp(cur: f64, prev: Option<f64>) -> Change {
    let _ = (cur, prev);
    Change::Unknown
}

impl Change {
    /// 화면 문구와 방향 (1 증가 / 0 변화 없음 / -1 감소)
    pub fn label(self) -> (String, i8) {
        ("".into(), 0)
    }

    /// CSS 클래스 (up / down / flat / new)
    pub fn class(self) -> &'static str {
        ""
    }

    /// 정렬 기준: 신규가 가장 크고, 비교할 수 없는 값이 가장 작다.
    fn rank(self) -> f64 {
        0.0
    }
}

/// 변화 열 정렬 (오름차순 기준)
pub fn cmp_change(a: Change, b: Change) -> Ordering {
    a.rank().total_cmp(&b.rank())
}

/// 목록이 잘리지 않고 전부 왔는지 (`limit`보다 적게 왔으면 전부)
pub fn is_complete(len: usize, limit: u32) -> bool {
    len < limit as usize
}

/// 현재 행과 이전 행을 키로 맞춘다. 반환: (현재 행, 이전 행)
///
/// - 이전에 없는 현재 행: 이전 목록이 전부 왔으면 0으로 본다(`zero` → 신규), 잘렸으면 None(모름).
/// - 현재에 없는 이전 행(사라짐): 현재 목록이 전부 왔으면 현재 값을 0으로 만들어 뒤에 붙인다.
/// - 순서: 현재 순서 그대로, 사라진 행은 이전 순서대로 뒤에. 같은 키가 여러 번 오면 처음 것만 쓴다.
pub fn pair<T: Clone>(
    cur: &[T],
    prev: &[T],
    key: impl Fn(&T) -> String,
    zero: impl Fn(&T) -> T,
    cur_complete: bool,
    prev_complete: bool,
) -> Vec<(T, Option<T>)> {
    let _ = (prev, &key, &zero, cur_complete, prev_complete);
    let _: HashMap<String, usize> = HashMap::new();
    cur.iter().map(|c| (c.clone(), None)).collect()
}

/// 상위 n개 + 나머지 "기타" (이전 값도 같이 합친다. 나머지 중 하나라도 모르면 기타의 이전 값도 모름)
pub fn top_with_rest_prev(
    rows: &[(String, i64, Option<i64>)],
    n: usize,
) -> Vec<(String, i64, Option<i64>)> {
    let _ = n;
    rows.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::table::{Sort, sort_rows};

    #[derive(Clone, PartialEq, Debug)]
    struct R {
        k: &'static str,
        v: u64,
    }
    fn r(k: &'static str, v: u64) -> R {
        R { k, v }
    }
    fn key(x: &R) -> String {
        x.k.to_string()
    }
    fn zero(x: &R) -> R {
        R { k: x.k, v: 0 }
    }

    #[test]
    fn percent_change_and_new() {
        assert_eq!(change(120.0, Some(100.0)), Change::Pct(0.2));
        assert_eq!(change(50.0, Some(100.0)), Change::Pct(-0.5));
        // 이전 0 → 신규 (∞%가 아니다)
        assert_eq!(change(5.0, Some(0.0)), Change::New);
        assert_eq!(change(0.0, Some(0.0)), Change::Unknown);
        assert_eq!(change(5.0, None), Change::Unknown);
        // 사라짐 = −100%
        assert_eq!(change(0.0, Some(8.0)), Change::Pct(-1.0));
        assert_eq!(change(f64::NAN, Some(1.0)), Change::Unknown);
    }

    #[test]
    fn labels() {
        assert_eq!(Change::Pct(0.123).label(), ("▲ 12.3%".into(), 1));
        assert_eq!(Change::Pct(-0.04).label(), ("▼ 4%".into(), -1));
        assert_eq!(Change::Pct(0.0).label(), ("–".into(), 0));
        assert_eq!(Change::New.label(), ("신규".into(), 1));
        assert_eq!(Change::Unknown.label(), ("–".into(), 0));
        assert!(!Change::New.label().0.contains('∞'));
        assert_eq!(Change::New.class(), "new");
        assert_eq!(Change::Pct(0.2).class(), "up");
        assert_eq!(Change::Pct(-0.2).class(), "down");
        assert_eq!(Change::Unknown.class(), "flat");
    }

    #[test]
    fn percentage_point_change_for_rates() {
        // 전환율 12.5% → 15.7%: +3.2%p (증감률 +25.6%가 아니다)
        let c = pp(0.157, Some(0.125));
        assert_eq!(c.label(), ("▲ 3.2%p".into(), 1));
        assert_eq!(pp(0.10, Some(0.125)).label(), ("▼ 2.5%p".into(), -1));
        assert_eq!(pp(0.3, Some(0.3)).label(), ("–".into(), 0));
        // 이전 비율이 0이어도 %p는 정의된다
        assert_eq!(pp(0.05, Some(0.0)).label(), ("▲ 5%p".into(), 1));
        assert_eq!(pp(0.05, None), Change::Unknown);
        assert_eq!(pp(f64::INFINITY, Some(0.1)), Change::Unknown);
        assert_eq!(pp(0.3, Some(0.1)).class(), "up");
    }

    #[test]
    fn pairs_rows_by_key_with_new_and_gone() {
        let cur = [r("naver", 10), r("google", 5), r("tiktok", 2)];
        let prev = [r("meta", 7), r("google", 4), r("naver", 12)];
        let p = pair(&cur, &prev, key, zero, true, true);
        assert_eq!(
            p,
            vec![
                (r("naver", 10), Some(r("naver", 12))),
                (r("google", 5), Some(r("google", 4))),
                // 이전에 없던 채널: 이전 0 → 신규
                (r("tiktok", 2), Some(r("tiktok", 0))),
                // 이번에 사라진 채널: 현재 0으로 뒤에 붙는다
                (r("meta", 0), Some(r("meta", 7))),
            ]
        );
        assert_eq!(
            change(p[2].0.v as f64, p[2].1.as_ref().map(|x| x.v as f64)),
            Change::New
        );
        assert_eq!(
            change(p[3].0.v as f64, p[3].1.as_ref().map(|x| x.v as f64)),
            Change::Pct(-1.0)
        );
    }

    #[test]
    fn truncated_lists_do_not_invent_new_or_gone_rows() {
        let cur = [r("/a", 10), r("/b", 5)];
        let prev = [r("/a", 8), r("/z", 3)];
        // 이전 목록이 잘렸으면 /b의 이전 값은 모른다, 현재 목록이 잘렸으면 /z를 붙이지 않는다
        let p = pair(&cur, &prev, key, zero, false, false);
        assert_eq!(p, vec![(r("/a", 10), Some(r("/a", 8))), (r("/b", 5), None)]);
        // 빈 쪽
        assert!(pair(&[], &[], key, zero, true, true).is_empty());
        assert_eq!(pair(&[], &prev, key, zero, true, true).len(), 2);
        // 같은 키가 두 번 오면 처음 것만
        let dup = [r("x", 1), r("x", 9)];
        assert_eq!(
            pair(&dup, &[r("x", 2)], key, zero, true, true),
            vec![(r("x", 1), Some(r("x", 2)))]
        );
    }

    #[test]
    fn sorting_by_change_is_stable_new_first_unknown_last() {
        let mut v = vec![
            ("a", Change::Pct(0.1)),
            ("b", Change::Unknown),
            ("c", Change::New),
            ("d", Change::Pct(0.1)),
            ("e", Change::Pct(-0.5)),
            ("f", Change::New),
        ];
        sort_rows(&mut v, Sort::desc(()), |a, b, _| cmp_change(a.1, b.1));
        let ks: Vec<&str> = v.iter().map(|x| x.0).collect();
        // 같은 값은 원래 순서 (c→f, a→d)
        assert_eq!(ks, ["c", "f", "a", "d", "e", "b"]);
    }

    #[test]
    fn rest_bucket_sums_previous_values() {
        let rows = vec![
            ("a".to_string(), 50, Some(40)),
            ("b".to_string(), 30, Some(35)),
            ("c".to_string(), 10, Some(5)),
            ("d".to_string(), 5, Some(0)),
            ("e".to_string(), 1, Some(2)),
        ];
        let v = top_with_rest_prev(&rows, 2);
        assert_eq!(v.len(), 3);
        assert_eq!(v[2], ("기타".to_string(), 16, Some(7)));
        let mut unknown = rows.clone();
        unknown[4].2 = None;
        assert_eq!(top_with_rest_prev(&unknown, 2)[2].2, None);
        // 하나만 남으면 묶지 않는다
        assert_eq!(top_with_rest_prev(&rows[..3], 2).len(), 3);
    }
}

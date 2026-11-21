//! 표 정렬·검색·CSV 내보내기.

use std::cmp::Ordering;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Asc,
    Desc,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sort<K> {
    pub key: K,
    pub dir: Dir,
}

impl<K: PartialEq + Copy> Sort<K> {
    pub fn desc(key: K) -> Self {
        Sort { key, dir: Dir::Desc }
    }

    /// 같은 열을 누르면 방향을 바꾸고, 다른 열이면 내림차순부터.
    pub fn toggle(self, key: K) -> Self {
        todo!()
    }

    /// 머리글 표시: ▼ / ▲ / 빈 문자열
    pub fn arrow(&self, key: K) -> &'static str {
        todo!()
    }

    /// `aria-sort` 값
    pub fn aria(&self, key: K) -> &'static str {
        todo!()
    }
}

/// 안정 정렬. `cmp`는 오름차순 기준 비교 함수.
pub fn sort_rows<T, K: Copy>(rows: &mut [T], s: Sort<K>, cmp: impl Fn(&T, &T, K) -> Ordering) {
    todo!()
}

/// 대소문자 무시 부분 일치 (유니코드). 빈 검색어는 모두 일치.
pub fn contains_ci(hay: &str, needle: &str) -> bool {
    todo!()
}

/// 문자열 비교 (대소문자 무시, 같으면 원문 순).
pub fn cmp_text(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b))
}

/// CSV (RFC 4180). 스프레드시트 수식 주입을 막기 위해 `= + - @`로 시작하는 문자열 칸 앞에 `'`를 붙인다
/// (숫자 칸은 그대로).
pub fn to_csv(headers: &[&str], rows: &[Vec<String>]) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum K {
        Name,
        N,
    }

    #[test]
    fn toggling() {
        let s = Sort::desc(K::N);
        assert_eq!(s.toggle(K::N).dir, Dir::Asc);
        assert_eq!(s.toggle(K::N).toggle(K::N).dir, Dir::Desc);
        let t = s.toggle(K::Name);
        assert_eq!((t.key, t.dir), (K::Name, Dir::Desc));
        assert_eq!(s.arrow(K::N), "▼");
        assert_eq!(s.toggle(K::N).arrow(K::N), "▲");
        assert_eq!(s.arrow(K::Name), "");
        assert_eq!(s.aria(K::N), "descending");
        assert_eq!(s.aria(K::Name), "none");
    }

    #[test]
    fn sorting_is_stable_both_ways() {
        let mut v = vec![("b", 2), ("a", 2), ("c", 1), ("가", 3)];
        let cmp = |x: &(&str, i32), y: &(&str, i32), k: K| match k {
            K::Name => cmp_text(x.0, y.0),
            K::N => x.1.cmp(&y.1),
        };
        sort_rows(&mut v, Sort::desc(K::N), cmp);
        assert_eq!(v, vec![("가", 3), ("b", 2), ("a", 2), ("c", 1)]);
        sort_rows(&mut v, Sort { key: K::N, dir: Dir::Asc }, cmp);
        assert_eq!(v, vec![("c", 1), ("b", 2), ("a", 2), ("가", 3)]);
        sort_rows(&mut v, Sort { key: K::Name, dir: Dir::Asc }, cmp);
        assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), ["a", "b", "c", "가"]);
        let mut empty: Vec<(&str, i32)> = vec![];
        sort_rows(&mut empty, Sort::desc(K::N), cmp);
    }

    #[test]
    fn search() {
        assert!(contains_ci("/Products/Shoes", "products"));
        assert!(contains_ci("/상품/신발", "신발"));
        assert!(contains_ci("anything", ""));
        assert!(contains_ci("anything", "   "));
        assert!(!contains_ci("/cart", "checkout"));
        assert!(contains_ci("ÄBC", "äb"));
    }

    #[test]
    fn csv_escaping() {
        let csv = to_csv(
            &["경로", "조회수"],
            &[
                vec!["/a,b".into(), "10".into()],
                vec!["say \"hi\"".into(), "-5".into()],
                vec!["=HYPERLINK(\"x\")".into(), "0".into()],
                vec!["line\nbreak".into(), "+3".into()],
            ],
        );
        assert_eq!(
            csv,
            "경로,조회수\r\n\"/a,b\",10\r\n\"say \"\"hi\"\"\",-5\r\n\"'=HYPERLINK(\"\"x\"\")\",0\r\n\"line\nbreak\",'+3\r\n"
        );
    }
}

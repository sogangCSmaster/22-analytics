//! 화면 상태 결정 로직 (컴포넌트는 얇게 두고, 여기서 무엇을 보여줄지·언제 초기화할지 정한다).

use dioxus::prelude::Key;

/// 특정 맥락(프로젝트·기간 등)에 묶어 저장한 값을 현재 맥락으로 읽는다.
///
/// 값은 `(저장할 때의 맥락, 값)`으로 들고 있다가, 맥락이 바뀌면 `None`(= 초기 상태)이 된다.
/// 효과(effect)로 나중에 지우는 방식과 달리, 프로젝트를 바꾼 바로 그 렌더에서 이전 값이 사라진다.
pub fn scoped<K: PartialEq, V: Clone>(stored: &Option<(K, V)>, key: &K) -> Option<V> {
    stored
        .as_ref()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

/// 목록 아래 쪽 넘김 표시.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PagerView {
    /// 1부터 (현재 쪽이 비었으면 0)
    pub from: u64,
    pub to: u64,
    pub total: u64,
    pub has_prev: bool,
    pub has_next: bool,
}

impl PagerView {
    pub fn label(&self) -> String {
        let total = crate::format::count(self.total);
        if self.from == 0 {
            format!("이 쪽에는 없어요 · 전체 {total}건")
        } else {
            format!("{}–{} / {total}건", self.from, self.to)
        }
    }
}

/// 전체 건수가 있으면 (현재 쪽이 비어 있어도) 쪽 넘김을 보여준다.
pub fn pager(offset: u32, rows: usize, total: u64, page: u32) -> Option<PagerView> {
    if total == 0 {
        return None;
    }
    let off = offset as u64;
    let (from, to) = if rows == 0 {
        (0, 0)
    } else {
        (off + 1, off + rows as u64)
    };
    Some(PagerView {
        from,
        to,
        total,
        has_prev: offset > 0,
        has_next: off + (page as u64) < total,
    })
}

/// 주기적 새로 고침. 탭이 숨겨진 동안은 부르지 않고, 다시 보이면 밀린 새로 고침을 바로 한다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Poller {
    pub interval_ms: u32,
    elapsed_ms: u32,
}

impl Poller {
    pub fn new(interval_ms: u32) -> Poller {
        Poller {
            interval_ms,
            elapsed_ms: 0,
        }
    }

    /// `step_ms`만큼 지났을 때 지금 새로 고칠지.
    pub fn tick(&mut self, step_ms: u32, hidden: bool) -> bool {
        // 숨겨진 동안에도 시간은 쌓아 두었다가, 다시 보이는 첫 틱에 바로 새로 고친다.
        self.elapsed_ms = self.elapsed_ms.saturating_add(step_ms);
        if hidden || self.elapsed_ms < self.interval_ms {
            return false;
        }
        self.elapsed_ms = 0;
        true
    }
}

/// 알림 한 개. 새 알림이 오면 세대(generation)가 바뀌어, 이전 알림의 타이머가 새 알림을 지우지 않는다.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ToastState {
    pub generation: u64,
    pub message: Option<String>,
}

impl ToastState {
    /// 새 알림을 띄우고 그 세대를 돌려준다.
    pub fn show(&mut self, msg: String) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.message = Some(msg);
        self.generation
    }

    /// `generation` 세대의 알림이 아직 떠 있으면 닫는다.
    pub fn expire(&mut self, generation: u64) {
        if self.generation == generation {
            self.message = None;
        }
    }
}

/// 버튼처럼 동작하는 행(tr)에서 실행 키인지 (Enter, Space)
pub fn is_activation_key(k: &Key) -> bool {
    match k {
        Key::Enter => true,
        Key::Character(c) => c == " ",
        _ => false,
    }
}

pub fn is_escape(k: &Key) -> bool {
    matches!(k, Key::Escape)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_value_resets_when_key_changes() {
        // 프로젝트 1에서 고른 주문은 프로젝트 2에서 보이지 않는다
        let sel = Some((1, "ORD-1".to_string()));
        assert_eq!(scoped(&sel, &1).as_deref(), Some("ORD-1"));
        assert_eq!(scoped(&sel, &2), None);
        assert_eq!(scoped::<i32, String>(&None, &1), None);
        // 쪽 번호는 (프로젝트, 기간) 묶음에 매인다
        let off = Some(((1, "2026-11-01"), 20u32));
        assert_eq!(scoped(&off, &(1, "2026-11-01")).unwrap_or(0), 20);
        assert_eq!(scoped(&off, &(1, "2026-10-01")).unwrap_or(0), 0);
        assert_eq!(scoped(&off, &(2, "2026-11-01")).unwrap_or(0), 0);
    }

    #[test]
    fn pager_shows_whenever_total_positive() {
        assert_eq!(pager(0, 0, 0, 10), None);
        let p = pager(0, 10, 25, 10).unwrap();
        assert_eq!((p.from, p.to, p.has_prev, p.has_next), (1, 10, false, true));
        assert_eq!(p.label(), "1–10 / 25건");
        let p = pager(20, 5, 25, 10).unwrap();
        assert_eq!(
            (p.from, p.to, p.has_prev, p.has_next),
            (21, 25, true, false)
        );
        // 현재 쪽이 비어도(주문이 줄어든 경우) 이전 쪽으로 돌아갈 수 있어야 한다
        let p = pager(30, 0, 25, 10).unwrap();
        assert!(p.has_prev && !p.has_next);
        assert_eq!(p.from, 0);
        assert_eq!(p.label(), "이 쪽에는 없어요 · 전체 25건");
        assert_eq!(pager(0, 3, 1_234, 10).unwrap().label(), "1–3 / 1,234건");
    }

    #[test]
    fn poller_pauses_while_hidden_and_catches_up() {
        let mut p = Poller::new(3_000);
        assert!(!p.tick(1_000, false));
        assert!(!p.tick(1_000, false));
        assert!(p.tick(1_000, false));
        // 숨겨진 동안은 부르지 않는다
        for _ in 0..10 {
            assert!(!p.tick(1_000, true));
        }
        // 다시 보이면 바로 새로 고친다
        assert!(p.tick(1_000, false));
        assert!(!p.tick(1_000, false));
    }

    #[test]
    fn older_toast_timer_does_not_close_newer_toast() {
        let mut t = ToastState::default();
        let a = t.show("A".into());
        let b = t.show("B".into());
        assert_ne!(a, b);
        t.expire(a);
        assert_eq!(t.message.as_deref(), Some("B"));
        t.expire(b);
        assert_eq!(t.message, None);
    }

    #[test]
    fn activation_and_escape_keys() {
        assert!(is_activation_key(&Key::Enter));
        assert!(is_activation_key(&Key::Character(" ".into())));
        assert!(!is_activation_key(&Key::Character("a".into())));
        assert!(!is_activation_key(&Key::Escape));
        assert!(is_escape(&Key::Escape));
        assert!(!is_escape(&Key::Enter));
    }
}

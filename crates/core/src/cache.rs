//! 크기 상한과 TTL이 있는 메모리 캐시 (Worker isolate, ingest 프로세스 공용).
//!
//! - 값은 `Option<V>`로 담는다: 없는 키(`None`)도 캐시해 저장소 조회를 줄인다 (negative caching).
//! - 시계는 호출자가 `now_ms`로 주입한다 (wasm에서도 쓰기 위해 내부에서 시간을 읽지 않는다).
//! - 가득 찬 상태에서 새 키가 들어오면 만료된 항목부터 비우고, 그래도 가득 차면 통째로 비운다.
//!   무작위 키로 메모리를 채우는 공격에도 크기가 `max_entries`를 넘지 않는다.
//! - 동기화는 하지 않는다. 여러 작업이 공유하면 호출자가 `Mutex` 등으로 감싼다.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone)]
pub struct TtlCache<K, V> {
    ttl_ms: i64,
    max_entries: usize,
    entries: HashMap<K, (i64, Option<V>)>,
}

impl<K: Eq + Hash, V: Clone> TtlCache<K, V> {
    pub fn new(ttl_ms: i64, max_entries: usize) -> Self {
        Self {
            ttl_ms,
            max_entries,
            entries: HashMap::new(),
        }
    }

    /// 신선한 항목이 있으면 `Some(값 또는 None)`, 없거나 만료됐으면 `None`.
    pub fn get<Q>(&self, key: &Q, now_ms: i64) -> Option<Option<V>>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.entries
            .get(key)
            .filter(|(at, _)| now_ms - at < self.ttl_ms)
            .map(|(_, v)| v.clone())
    }

    pub fn put(&mut self, key: K, value: Option<V>, now_ms: i64) {
        if self.entries.len() >= self.max_entries && !self.entries.contains_key(&key) {
            let ttl = self.ttl_ms;
            self.entries.retain(|_, (at, _)| now_ms - *at < ttl);
            if self.entries.len() >= self.max_entries {
                self.entries.clear();
            }
        }
        self.entries.insert(key, (now_ms, value));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expires_after_ttl() {
        let mut c: TtlCache<String, u32> = TtlCache::new(60_000, 10);
        assert_eq!(c.get("a", 0), None);
        c.put("a".into(), Some(1), 1_000);
        assert_eq!(c.get("a", 1_000), Some(Some(1)));
        assert_eq!(c.get("a", 60_999), Some(Some(1)));
        assert_eq!(c.get("a", 61_000), None, "TTL이 지나면 다시 읽는다");
        // 다시 넣으면 시각이 갱신된다
        c.put("a".into(), Some(2), 61_000);
        assert_eq!(c.get("a", 100_000), Some(Some(2)));
    }

    #[test]
    fn caches_negative_entries() {
        let mut c: TtlCache<u32, String> = TtlCache::new(1_000, 10);
        c.put(7, None, 0);
        assert_eq!(c.get(&7, 10), Some(None), "없는 키도 캐시");
        assert_eq!(c.get(&7, 1_000), None, "음성 항목도 만료된다");
        assert_eq!(c.get(&8, 10), None);
    }

    #[test]
    fn bounded_capacity() {
        let mut c: TtlCache<String, ()> = TtlCache::new(60_000, 3);
        for i in 0..3 {
            c.put(format!("k{i}"), None, 0);
        }
        assert_eq!(c.len(), 3);
        // 이미 있는 키를 갱신할 때는 비우지 않는다
        c.put("k0".into(), Some(()), 10);
        assert_eq!(c.len(), 3);
        // 만료된 항목부터 비운다
        c.put("new".into(), None, 60_005);
        assert_eq!(c.len(), 2, "k0(10)만 남고 k1,k2(0)는 만료");
        assert_eq!(c.get("k0", 60_005), Some(Some(())));
        // 모두 신선한데 가득 차면 통째로 비우고 새로 담는다
        for i in 0..10_000 {
            c.put(format!("x{i}"), None, 60_005);
            assert!(c.len() <= 3);
        }
        assert!(!c.is_empty());
    }
}

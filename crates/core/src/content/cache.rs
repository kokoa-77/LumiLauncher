//! 搜索结果、项目详情的短时缓存：同样的请求在有效期内直接用上次的结果。
//! 资源中心来回切分类、从详情页返回、重新打开同一个项目都不用再等网络（D17 的目标：有缓存时 < 300 毫秒）。
//! 只在内存里，关掉启动器就没了；平台上的数据会变，所以有效期不长。

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub struct TtlCache<V> {
    ttl: Duration,
    capacity: usize,
    entries: Mutex<HashMap<String, (Instant, V)>>,
}

impl<V: Clone> TtlCache<V> {
    /// `ttl`：有效期；`capacity`：最多存几条（满了先丢过期的，再丢最旧的）
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            ttl,
            capacity: capacity.max(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, (Instant, V)>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 还在有效期内的结果；过期的顺手删掉
    pub fn get(&self, key: &str) -> Option<V> {
        let mut entries = self.lock();
        match entries.get(key) {
            Some((at, value)) if at.elapsed() < self.ttl => Some(value.clone()),
            Some(_) => {
                entries.remove(key);
                None
            }
            None => None,
        }
    }

    pub fn insert(&self, key: String, value: V) {
        let mut entries = self.lock();
        if entries.len() >= self.capacity && !entries.contains_key(&key) {
            let ttl = self.ttl;
            entries.retain(|_, (at, _)| at.elapsed() < ttl);
            if entries.len() >= self.capacity
                && let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, (at, _))| *at)
                    .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(key, (Instant::now(), value));
    }

    /// 清空（比如换了游戏目录、实例变了时）
    pub fn clear(&self) {
        self.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_within_ttl_and_misses_after() {
        let cache = TtlCache::new(Duration::from_secs(60), 10);
        cache.insert("a".into(), 1);
        assert_eq!(cache.get("a"), Some(1));
        assert_eq!(cache.get("b"), None);

        let expired = TtlCache::new(Duration::ZERO, 10);
        expired.insert("a".into(), 1);
        assert_eq!(expired.get("a"), None);
    }

    #[test]
    fn full_cache_drops_the_oldest() {
        let cache = TtlCache::new(Duration::from_secs(60), 2);
        cache.insert("a".into(), 1);
        std::thread::sleep(Duration::from_millis(2));
        cache.insert("b".into(), 2);
        std::thread::sleep(Duration::from_millis(2));
        cache.insert("c".into(), 3);
        assert_eq!(cache.get("a"), None);
        assert_eq!(cache.get("b"), Some(2));
        assert_eq!(cache.get("c"), Some(3));
        // 覆盖已有的键不算新增
        cache.insert("c".into(), 4);
        assert_eq!(cache.get("b"), Some(2));
        assert_eq!(cache.get("c"), Some(4));
        cache.clear();
        assert_eq!(cache.get("c"), None);
    }
}

use std::{
    collections::HashMap,
    hash::Hash,
    time::{Duration, Instant},
};

/// 定义了一个基于时间的缓存结构，用于存储键值对。
/// 键和值都需要实现可比较（Eq）、哈希（Hash）和克隆（Clone）的特性。
/// 缓存中的条目在设定的持续时间过后将被视为过期。
pub struct Cache<K: Eq + Hash + Clone, V: Clone> {
    /// 缓存条目的过期时间。
    duration: Duration,
    /// 用于记录每个键的最后访问时间。
    instants: HashMap<K, Instant>,
    /// 存储实际的缓存值。
    values: HashMap<K, V>,
}

/// 实现了Cache结构体的构造方法。
impl<K: Eq + Hash + Clone, V: Clone> Cache<K, V> {
    /// 创建一个新的Cache实例，指定缓存条目的过期时间。
    pub fn new(duration: Duration) -> Self {
        Cache {
            duration,
            instants: Default::default(),
            values: Default::default(),
        }
    }

    /// 从缓存中获取指定键的值。
    /// 如果条目不存在或已过期，则返回None。
    pub fn get(&self, key: K) -> Option<V> {
        let instant = self.instants.get(&key)?;
        if instant.elapsed() > self.duration {
            return None;
        }
        self.values.get(&key).cloned()
    }

    /// 向缓存中添加一个新的键值对。
    /// 同时更新对应键的最后访问时间。
    pub fn set(&mut self, key: K, value: V) {
        self.values.insert(key.clone(), value);
        self.instants.insert(key, Instant::now());
    }
}

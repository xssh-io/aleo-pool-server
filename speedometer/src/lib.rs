use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use tokio::sync::RwLock;

/// 速度计是一种用于计算事件发生速度的结构体。
/// 它使用时间窗口来存储事件发生的时间和值，并根据这些信息计算事件的速度。
pub struct Speedometer {
    /// 用于存储事件发生时间和值的双端队列。
    /// 使用读写锁来确保并发访问的安全性。
    storage: RwLock<VecDeque<(Instant, u64)>>,
    /// 计算速度的时间窗口间隔。
    interval: Duration,
    /// 标志是否启用缓存速度值。
    cached: bool,
    /// 缓存速度值的有效期。
    cache_interval: Option<Duration>,
    /// 上次计算速度的时间点。
    cache_instant: Option<Instant>,
    /// 缓存的速度值。
    cache_value: f64,
}

impl Speedometer {

    /// 初始化速度计，不启用缓存。
    pub fn init(interval: Duration) -> Self {
        Self {
            storage: RwLock::new(VecDeque::new()),
            interval,
            cached: false,
            cache_interval: None,
            cache_instant: None,
            cache_value: 0.0,
        }
    }

    /// 初始化速度计，并启用缓存。
    pub fn init_with_cache(interval: Duration, cache_interval: Duration) -> Self {
        Self {
            storage: RwLock::new(VecDeque::new()),
            interval,
            cached: true,
            cache_interval: Some(cache_interval),
            cache_instant: Some(Instant::now() - cache_interval),
            cache_value: 0.0,
        }
    }

    /// 记录一个事件的发生，更新速度计的存储。
    pub async fn event(&self, value: u64) {
        let mut storage = self.storage.write().await;
        storage.push_back((Instant::now(), value));
        // 保持时间窗口的大小，移除超出时间间隔的旧事件。
        while storage.front().map_or(false, |t| t.0.elapsed() > self.interval) {
            storage.pop_front();
        }
    }

    /// 计算当前的事件发生速度。
    pub async fn speed(&mut self) -> f64 {
        // 如果启用了缓存且缓存还未过期，则直接返回缓存的值。
        if self.cached && self.cache_instant.unwrap().elapsed() < self.cache_interval.unwrap() {
            return self.cache_value;
        }
        let mut storage = self.storage.write().await;
        // 保持时间窗口的大小，移除超出时间间隔的旧事件。
        while storage.front().map_or(false, |t| t.0.elapsed() > self.interval) {
            storage.pop_front();
        }
        drop(storage);
        // 计算时间窗口内事件的总数。
        let events = self.storage.read().await.iter().fold(0, |acc, t| acc + t.1);
        let speed = events as f64 / self.interval.as_secs_f64();
        // 如果启用了缓存，更新缓存值和缓存时间。
        if self.cached {
            self.cache_instant = Some(Instant::now());
            self.cache_value = speed;
        }
        speed
    }

    /// 重置速度计，清除所有事件记录。
    #[allow(dead_code)]
    pub async fn reset(&self) {
        self.storage.write().await.clear();
    }
}

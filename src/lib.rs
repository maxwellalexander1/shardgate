//! ShardGate v0 — core limiter + idle cleanup sweeper.
//!
//! Think arcade cards: each key has its own card with `capacity` tokens.
//! Each `check()` spends 1 token. Tokens come back over time (lazy refill).
//! Forgotten cards are tossed by the sweeper after `idle_after`.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Config for the limiter.
#[derive(Debug, Clone)]
pub struct Config {
    /// Max tokens a bucket can hold.
    pub capacity: u32,
    /// How fast tokens come back, per second.
    pub refill_rate: f64,
    /// Number of shards (lockers). Default 16.
    pub num_shards: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            capacity: 10,
            refill_rate: 1.0,
            num_shards: 16,
        }
    }
}

/// What `check()` returns.
#[derive(Debug, PartialEq)]
pub enum Decision {
    Allowed,
    Denied { retry_after: Duration },
}

/// One user's bucket: current tokens + when we last added tokens.
#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

impl Bucket {
    fn new_full(capacity: u32, now: Instant) -> Self {
        Self {
            tokens: capacity as f64,
            last_refill: now,
        }
    }

    /// Add tokens based on time passed, capped at capacity.
    /// Returns true (Allowed) if we could take 1 token, false (Denied) if not.
    fn try_take(&mut self, now: Instant, config: &Config) -> bool {
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        if elapsed > 0.0 {
            self.tokens += elapsed * config.refill_rate;
            if self.tokens > config.capacity as f64 {
                self.tokens = config.capacity as f64;
            }
            self.last_refill = now;
        }
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// How long until 1 full token exists?
    fn retry_after(&self, config: &Config) -> Duration {
        if config.refill_rate <= 0.0 {
            // Never refills — tell caller to wait a long time.
            return Duration::from_secs(60);
        }
        let needed = 1.0 - self.tokens;
        if needed <= 0.0 {
            Duration::from_secs(0)
        } else {
            Duration::from_secs_f64(needed / config.refill_rate)
        }
    }
    /// True if nobody touched this bucket for longer than `idle_after`.
    /// Safe to delete either way: lazy refill would make it full on next check.
    fn is_idle(&self, now: Instant, idle_after: Duration) -> bool {
        now.duration_since(self.last_refill) > idle_after
    }
}

/// One shard = its own lock + map. Many keys share one shard.
struct Shard {
    inner: Mutex<HashMap<String, Bucket>>,
}

/// The main limiter. Share with `Arc<RateLimiter>` across threads/tasks.
pub struct RateLimiter {
    config: Config,
    shards: Vec<Shard>,
}

impl RateLimiter {
    pub fn new(config: Config) -> Self {
        assert!(config.num_shards > 0, "num_shards must be > 0");
        let mut shards = Vec::with_capacity(config.num_shards);
        for _ in 0..config.num_shards {
            shards.push(Shard {
                inner: Mutex::new(HashMap::new()),
            });
        }
        Self { config, shards }
    }

    fn shard_index(&self, key: &str) -> usize {
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        (h.finish() as usize) % self.shards.len()
    }

    /// Is this caller allowed right now?
    pub fn check(&self, key: &str) -> Decision {
        let idx = self.shard_index(key);
        let now = Instant::now();

        // Lock ONLY this shard — other shards keep running in parallel.
        let mut map = self.shards[idx].inner.lock().unwrap();

        let bucket = map
            .entry(key.to_string())
            .or_insert_with(|| Bucket::new_full(self.config.capacity, now));

        if bucket.try_take(now, &self.config) {
            Decision::Allowed
        } else {
            Decision::Denied {
                retry_after: bucket.retry_after(&self.config),
            }
        }
    }

    /// Remove all buckets idle longer than `idle_after`. Returns count removed.
    /// Sync + testable — the async sweeper just calls this on a timer.
    pub fn cleanup_once(&self, idle_after: Duration) -> usize {
        let now = Instant::now();
        let mut removed = 0;
        // One shard at a time, lock released before next shard. Never across .await.
        for shard in &self.shards {
            let mut map = shard.inner.lock().unwrap();
            let before = map.len();
            map.retain(|_, b| !b.is_idle(now, idle_after));
            removed += before - map.len();
        }
        removed
    }

    /// How many buckets exist right now? Sum len() one shard at a time.
    /// Used by /metrics. Rare call, so brief per-shard locks are fine.
    pub fn bucket_count(&self) -> usize {
        self.shards
            .iter()
            .map(|s| s.inner.lock().unwrap().len())
            .sum()
    }

    /// Background sweeper: every `interval`, drop idle buckets.
    /// Explicit (not auto in new()) for reliability — caller opts in with one line.
    /// Returns handle you can abort(). Locks never held across `.await`.
    pub fn spawn_cleanup(
        self: &Arc<Self>,
        interval: Duration,
        idle_after: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                me.cleanup_once(idle_after);
            }
        })
    }
}

/// Naive baseline: ONE lock for all keys.
/// Same buckets, same refill, same Decision — only locking differs.
/// Think: 1 giant locker everyone queues for. Used to prove sharding wins.
pub struct NaiveLimiter {
    config: Config,
    inner: Mutex<HashMap<String, Bucket>>,
}

impl NaiveLimiter {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Same interface as RateLimiter::check — fair fight.
    pub fn check(&self, key: &str) -> Decision {
        let now = Instant::now();
        let mut map = self.inner.lock().unwrap();
        let bucket = map
            .entry(key.to_string())
            .or_insert_with(|| Bucket::new_full(self.config.capacity, now));
        if bucket.try_take(now, &self.config) {
            Decision::Allowed
        } else {
            Decision::Denied {
                retry_after: bucket.retry_after(&self.config),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(capacity: u32, refill_rate: f64) -> Config {
        Config {
            capacity,
            refill_rate,
            num_shards: 4,
        }
    }

    #[test]
    fn starts_full_allows_capacity_requests() {
        let lim = RateLimiter::new(test_config(3, 1.0));
        assert_eq!(lim.check("alice"), Decision::Allowed);
        assert_eq!(lim.check("alice"), Decision::Allowed);
        assert_eq!(lim.check("alice"), Decision::Allowed);
        // 4th should be denied — bucket empty now.
        match lim.check("alice") {
            Decision::Denied { .. } => {}
            d => panic!("expected Denied, got {:?}", d),
        }
    }

    #[test]
    fn different_keys_dont_share_tokens() {
        let lim = RateLimiter::new(test_config(1, 1.0));
        assert_eq!(lim.check("alice"), Decision::Allowed);
        // alice empty, but bob has his own full bucket.
        assert_eq!(lim.check("bob"), Decision::Allowed);
    }

    #[test]
    fn refill_after_time_allows_again() {
        let lim = RateLimiter::new(test_config(1, 10.0)); // 1 token per 0.1s
        assert_eq!(lim.check("alice"), Decision::Allowed);
        match lim.check("alice") {
            Decision::Denied { retry_after } => {
                assert!(retry_after.as_secs_f64() > 0.0);
                assert!(retry_after.as_secs_f64() < 0.5);
            }
            d => panic!("expected Denied, got {:?}", d),
        }
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(lim.check("alice"), Decision::Allowed);
    }

    #[test]
    fn cleanup_removes_idle_buckets() {
        let lim = RateLimiter::new(test_config(2, 1.0));
        assert_eq!(lim.check("alice"), Decision::Allowed);
        assert_eq!(lim.check("bob"), Decision::Allowed);
        // Nothing old enough yet — nothing removed.
        assert_eq!(lim.cleanup_once(Duration::from_secs(300)), 0);
        // Everything idle > 0ms — both removed. Next check makes fresh full bucket.
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(lim.cleanup_once(Duration::from_millis(1)), 2);
        assert_eq!(lim.check("alice"), Decision::Allowed);
    }

    #[test]
    fn naive_matches_sharded_for_basic_flow() {
        // Same rules: full at start, isolates keys, denies when empty.
        let naive = NaiveLimiter::new(test_config(1, 1.0));
        assert_eq!(naive.check("alice"), Decision::Allowed);
        assert_eq!(naive.check("bob"), Decision::Allowed);
        match naive.check("alice") {
            Decision::Denied { .. } => {}
            d => panic!("expected Denied, got {:?}", d),
        }
    }
}

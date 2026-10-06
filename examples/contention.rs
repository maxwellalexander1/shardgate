//! Contention proof: same key (1 lock) vs spread keys (16 locks).
//! Run: cargo run --release --example contention
//! Uses std threads for true parallel hammer — no async overhead.

use shardgate::{Config, RateLimiter};
use std::sync::Arc;
use std::time::Instant;

const THREADS: usize = 8;
const PER_THREAD: usize = 50_000; // 8 x 50k = 400k total

fn make_limiter() -> Arc<RateLimiter> {
    Arc::new(RateLimiter::new(Config {
        capacity: 1_000_000, // huge so we never deny — we time locks only
        refill_rate: 1_000_000.0,
        num_shards: 16,
    }))
}

fn run_hot() -> u128 {
    let limiter = make_limiter();
    let start = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..THREADS {
        let l = limiter.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..PER_THREAD {
                l.check("hot_key");
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    start.elapsed().as_millis()
}

fn run_spread() -> u128 {
    let limiter = make_limiter();
    let start = Instant::now();
    let mut handles = Vec::new();
    for t in 0..THREADS {
        let l = limiter.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..PER_THREAD {
                // Unique key per op spreads across 16 shards.
                let key = format!("user-{}-{}", t, i);
                l.check(&key);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    start.elapsed().as_millis()
}

fn main() {
    let total = (THREADS * PER_THREAD) as f64;

    let hot_ms = run_hot();
    println!(
        "HOT single key:   {} ops in {} ms = {:.0} ops/sec (1 lock, queued)",
        THREADS * PER_THREAD,
        hot_ms,
        total / (hot_ms.max(1) as f64 / 1000.0)
    );

    let spread_ms = run_spread();
    println!(
        "SPREAD many keys: {} ops in {} ms = {:.0} ops/sec (16 locks, parallel)",
        THREADS * PER_THREAD,
        spread_ms,
        total / (spread_ms.max(1) as f64 / 1000.0)
    );

    let gap = hot_ms.max(1) as f64 / spread_ms.max(1) as f64;
    if gap >= 1.0 {
        println!(
            "Gap: SPREAD was {:.2}x faster than HOT — sharding wins.",
            gap
        );
    } else {
        println!(
            "Gap: HOT was {:.2}x faster (unexpected on this machine — spread creates many HashMap entries, try --release).",
            1.0 / gap
        );
    }
}

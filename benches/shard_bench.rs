//! Baseline vs sharded: HOT 1 key vs SPREAD many keys, x 1/2/4/8 threads.
//! Run: cargo bench
//! Same shape as before, just adds thread sweep + naive 1-lock comparison.

use criterion::{criterion_group, criterion_main, Criterion};
use shardgate::{Config, NaiveLimiter, RateLimiter};
use std::sync::Arc;

const PER_THREAD: usize = 5_000;
const SPREAD_POOL: usize = 256; // rotating keys, map stays bounded

fn sharded_config() -> Config {
    Config {
        capacity: 1_000_000, // huge so we never deny — we time locks only
        refill_rate: 1_000_000.0,
        num_shards: 16,
    }
}

fn naive_config() -> Config {
    Config {
        capacity: 1_000_000,
        refill_rate: 1_000_000.0,
        num_shards: 16, // ignored by naive, kept so Config stays identical
    }
}

// Real helpers using Arc (thread-safe share):
fn hot_batch_sharded(lim: Arc<RateLimiter>, threads: usize) {
    let mut hs = Vec::with_capacity(threads);
    for _ in 0..threads {
        let l = lim.clone();
        hs.push(std::thread::spawn(move || {
            for _ in 0..PER_THREAD {
                l.check("hot_key");
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
}

fn hot_batch_naive(lim: Arc<NaiveLimiter>, threads: usize) {
    let mut hs = Vec::with_capacity(threads);
    for _ in 0..threads {
        let l = lim.clone();
        hs.push(std::thread::spawn(move || {
            for _ in 0..PER_THREAD {
                l.check("hot_key");
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
}

fn spread_batch_sharded(lim: Arc<RateLimiter>, threads: usize) {
    let mut hs = Vec::with_capacity(threads);
    for t in 0..threads {
        let l = lim.clone();
        hs.push(std::thread::spawn(move || {
            for i in 0..PER_THREAD {
                // Bounded pool spreads across shards without growing memory.
                let k = (t * PER_THREAD + i) % SPREAD_POOL;
                // Avoid format! in hot loop — use small static-ish keys via pool index.
                // Simple match-free approach: pre-made strings would be faster,
                // but format! cost is equal for naive vs sharded, so fight stays fair.
                l.check(&format!("user_{}", k));
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
}

fn spread_batch_naive(lim: Arc<NaiveLimiter>, threads: usize) {
    let mut hs = Vec::with_capacity(threads);
    for t in 0..threads {
        let l = lim.clone();
        hs.push(std::thread::spawn(move || {
            for i in 0..PER_THREAD {
                let k = (t * PER_THREAD + i) % SPREAD_POOL;
                l.check(&format!("user_{}", k));
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
}

fn bench_matrix(c: &mut Criterion) {
    for threads in [1usize, 2, 4, 8] {
        // HOT — one key, everyone queues on one lock either way.
        {
            let lim = Arc::new(RateLimiter::new(sharded_config()));
            c.bench_function(&format!("hot/sharded/{}t", threads), |b| {
                b.iter(|| hot_batch_sharded(lim.clone(), threads))
            });
        }
        {
            let lim = Arc::new(NaiveLimiter::new(naive_config()));
            c.bench_function(&format!("hot/naive/{}t", threads), |b| {
                b.iter(|| hot_batch_naive(lim.clone(), threads))
            });
        }
        // SPREAD — many keys: 16 locks run parallel, 1 lock still queues.
        {
            let lim = Arc::new(RateLimiter::new(sharded_config()));
            c.bench_function(&format!("spread/sharded/{}t", threads), |b| {
                b.iter(|| spread_batch_sharded(lim.clone(), threads))
            });
        }
        {
            let lim = Arc::new(NaiveLimiter::new(naive_config()));
            c.bench_function(&format!("spread/naive/{}t", threads), |b| {
                b.iter(|| spread_batch_naive(lim.clone(), threads))
            });
        }
    }
}

criterion_group!(benches, bench_matrix);
criterion_main!(benches);

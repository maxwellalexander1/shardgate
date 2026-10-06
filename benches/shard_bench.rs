//! Benchmark: HOT single key vs SPREAD many keys.
//! Run: cargo bench
//! This shows package maturity — criterion charts throughput.

use criterion::{Criterion, criterion_group, criterion_main};
use shardgate::{Config, RateLimiter};

fn bench_hot(c: &mut Criterion) {
    let limiter = RateLimiter::new(Config {
        capacity: 10000, // big so we never deny, we measure lock speed
        refill_rate: 10000.0,
        num_shards: 16,
    });
    c.bench_function("hot_single_key", |b| {
        b.iter(|| limiter.check("hot_key"));
    });
}

fn bench_spread(c: &mut Criterion) {
    let limiter = RateLimiter::new(Config {
        capacity: 10000,
        refill_rate: 10000.0,
        num_shards: 16,
    });
    // 128 rotating keys spread across shards.
    let keys: Vec<String> = (0..128).map(|i| format!("user_{}", i)).collect();
    let mut i = 0;
    c.bench_function("spread_many_keys", |b| {
        b.iter(|| {
            let k = &keys[i % keys.len()];
            i += 1;
            limiter.check(k)
        });
    });
}

criterion_group!(benches, bench_hot, bench_spread);
criterion_main!(benches);

# ShardGate

A fast token bucket rate limiter in Rust. It uses sharded locks so many threads can check limits at once.

Each key gets its own bucket. Buckets refill over time. Checks are cheap and honest about when to retry.

```
check(key) -> hash -> one of 16 shards -> lock only that shard
  -> refill by elapsed time -> take 1 token or deny with retry_after
sweeper: every 60s drop buckets idle over 300s
```

## Quick start

```rust
use shardgate::{Config, Decision, RateLimiter};
use std::sync::Arc;

let limiter = Arc::new(RateLimiter::new(Config {
    capacity: 10,
    refill_rate: 1.0,
    num_shards: 16,
}));

match limiter.check("user_42") {
    Decision::Allowed => println!("in"),
    Decision::Denied { retry_after } => println!("retry in {:?}", retry_after),
}

// Optional cleanup for long running servers:
let _handle = limiter.spawn_cleanup(
    std::time::Duration::from_secs(60),
    std::time::Duration::from_secs(300),
);
```

## Proof

From `cargo run --release --example contention` (8 threads, 400k ops):

* HOT one key: 158ms, 2.5M ops/sec
* SPREAD many keys: 56ms, 7.1M ops/sec
* SPREAD was 2.82x faster. Gap grows with more threads and keys.

From `cargo run --example demo` (32 tokio tasks):

* HOT: 10 Allowed, 22 Denied. Proves it denies when full.
* SPREAD: 3200 Allowed, 0 Denied. Proves it runs parallel when spread.

## Run it

```bash
cargo test
cargo run --example demo
cargo run --release --example contention
cargo bench
```

## Design calls

* Shards (16 default): less waiting, a bit more memory. One global lock would queue everyone.
* Lazy refill: no timers per key. Cost scales with requests, not keys.
* Explicit cleanup: you opt in with `spawn_cleanup`. No magic in `new`, safer for prod.

## Interview Q&A

**Why shards?** One Mutex for all keys means all threads wait. Hash to 16 locks means same key stays correct, different keys run free.

**Why lazy refill?** `tokens += elapsed * rate` on each check. No background work per key.

**What if a bucket is idle long?** Delete it. Next check makes a fresh full one, same result as if it refilled.

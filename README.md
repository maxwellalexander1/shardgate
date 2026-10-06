# ShardGate

![CI](https://github.com/maxwellalexander1/shardgate/workflows/CI/badge.svg)

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

## Architecture

```
GET /check?key=X -> hash(X) % 16 -> lock 1 shard
  -> get-or-create bucket -> lazy refill (tokens += elapsed * rate, cap)
  -> Allowed (take 1) or Denied + retry_after
GET /healthz -> 200 ok (no lock)
GET /metrics -> 2 atomics + sum len per shard
sweeper: tokio task every 60s, drops buckets idle > 300s, one shard at a time
```

* Shared via `Arc<RateLimiter>` across tasks. `std::Mutex` is fine — held briefly, never across `.await`.
* 16 shards default: less contention, a bit more memory. Same key always same shard.

## Service API

| Method | Example | Success | Limited | Other |
|--------|---------|---------|---------|-------|
| `GET /check?key=<k>` | `curl "localhost:8080/check?key=alice"` | `200 {"key":"alice","allowed":true}` | `429 + Retry-After: N + {"key":"alice","allowed":false,"retry_after_secs":N}` | `400 {"error":"missing ?key="}` |
| `GET /healthz` | `curl localhost:8080/healthz` | `200 ok` | — | — |
| `GET /metrics` | `curl localhost:8080/metrics` | `200 allowed_total N\ndenied_total N\nactive_buckets N` | — | — |

Env (`SHARDGATE_CAPACITY=10` `SHARDGATE_REFILL_RATE=1.0` `SHARDGATE_SHARDS=16` `PORT=8080`). `Retry-After` is whole seconds, rounded up, min 1.

## Proof

Load test (`wrk -t4 -c64 -d30s --latency`, release server, CachyOS 16-core 31GB, client+server shared so noisy/conservative):

| config | pattern | req/s | p50 | p99 | allowed | denied | buckets |
|--------|---------|-------|-----|-----|---------|--------|---------|
| defaults cap10/refill1 | HOT 1 key | 647k | 61us | 339us | 40 | 19.48M | 1 |
| defaults cap10/refill1 | SPREAD 256 keys | 638k | 60us | 333us | 10,123 | 19.14M | 256 |
| huge cap1M/refill1M | HOT 1 key | 646k | 61us | 335us | 19.45M | 0 | 1 |
| huge cap1M/refill1M | SPREAD 256 keys | 606k | 65us | 352us | 18.23M | 0 | 256 |

* Defaults show real limiting (mostly 429 + Retry-After). Huge shows ceiling (all 200).
* Over HTTP HOT ~= SPREAD (HTTP/JSON dominates, unlike in-process bench where locks dominate) — honest result.
* Run: `wrk_hot.lua` fixed key, `wrk_spread.lua` random user_0..255. Metrics delta from `/metrics`.

From `cargo bench` (sharded 16 locks vs naive 1 lock, 5k ops per thread, release):

| test | 1t | 2t | 4t | 8t |
|------|----|----|----|----|
| HOT sharded | 1.01 ms | 2.15 ms | 4.63 ms | 11.54 ms |
| HOT naive | 0.67 ms | 1.90 ms | 3.85 ms | 11.11 ms |
| SPREAD sharded | 1.02 ms | 1.07 ms | 1.26 ms | 2.46 ms |
| SPREAD naive | 0.87 ms | 2.74 ms | 5.47 ms | 11.38 ms |

* At 1 thread naive wins (no hash overhead) — expected.
* HOT ties at 8t (one key = one lock either way).
* SPREAD sharded wins big: 2.56x at 2t, 4.34x at 4t, 4.63x at 8t (16.3M vs 3.5M ops/sec).

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

### Service

```bash
cargo run --bin shardgate-server
curl localhost:8080/healthz
curl "localhost:8080/check?key=alice"
curl localhost:8080/metrics
```

Env: `SHARDGATE_CAPACITY=10` `SHARDGATE_REFILL_RATE=1.0` `SHARDGATE_SHARDS=16` `PORT=8080`.

### Run with Docker (138MB)

```bash
docker build -t shardgate .
docker run -p 8080:8080 shardgate
curl localhost:8080/healthz
```

Multi-stage (rust builder + debian-slim runner), non-root `appuser`, `HEALTHCHECK` on `/healthz`.

## Limitations (honest PoC)

* Single node, in-memory: restart wipes state, two servers don't share limits.
* Per-process, not global: Alice gets 10 on A + 10 on B. Global needs consistent-hash routing or shared store.
* No persistence: cards live only in HashMaps + sweeper.
* Hot key still queues: one key = one shard lock (see HOT tie in bench).
* Sweeper can lag: if idle buckets pile faster than 60s sweep, memory grows until sweep catches up.
* Containerized, not deployed: Docker image runs locally; no cloud host yet.

## Interview Q&A

**Why shards?** One Mutex for all keys means all threads wait. Hash to 16 locks means same key stays correct, different keys run free.

**Why lazy refill?** `tokens += elapsed * rate` on each check. No background work per key.

**What if a bucket is idle long?** Delete it. Next check makes a fresh full one, same result as if it refilled.

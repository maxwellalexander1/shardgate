# ShardGate: Design

## What it is
A rate limiter library in Rust that answers one question: "is this caller allowed to make a request right now?" It uses the token-bucket algorithm, and it splits its state into shards so many threads can check limits at once without fighting over a single lock.

## Core idea: token bucket
Each caller (identified by a key such as an API key or IP) has a bucket that holds up to `capacity` tokens. Tokens refill at `refill_rate` per second. A request costs 1 token. If a token is available, the request is allowed and a token is removed. Otherwise it is rejected.

Refill is **lazy**: there is no timer per bucket. When a request arrives, the bucket works out how much time has passed since `last_refill`, adds the matching tokens (capped at capacity), then tries to take one.

## Why shards
One global `Mutex<HashMap>` means every request from every caller waits on the same lock. Instead, ShardGate keeps `N` independent shards, each with its own lock. A key is hashed to pick its shard, so requests for different keys usually touch different locks and run in parallel. Requests for the same key always land on the same shard, so that key's bucket stays consistent.

## Components
- **`Bucket`**: holds `tokens` and `last_refill`. Method: `try_take(now, config) -> bool`.
- **`Shard`**: a `Mutex<HashMap<Key, Bucket>>`.
- **`RateLimiter`**: owns a `Vec<Shard>` and the config (`capacity`, `refill_rate`, `num_shards`). Public API: `check(key) -> Decision`.
- **`Decision`**: `Allowed` or `Denied { retry_after }`.
- **Cleanup task** (Tokio): a background task that periodically sweeps each shard and removes idle buckets, so memory does not grow forever.

## Request flow
1. Caller invokes `limiter.check("user_42")`.
2. Hash the key, then `shard_index = hash % num_shards`.
3. Lock that shard only.
4. Get or create the bucket for the key (new buckets start full).
5. Refill based on elapsed time, then try to take a token.
6. Unlock and return `Allowed` or `Denied`.

## Where Tokio fits
- The limiter is shared across tasks via `Arc<RateLimiter>`.
- The cleanup sweeper runs as a `tokio::spawn` task with `tokio::time::interval`.
- The lock is held only for a tiny, non-blocking critical section, so a plain `std::sync::Mutex` is fine (never hold it across an `.await`).

## Prototype scope (v0)
- **In:** the bucket, sharded map, `check()`, the cleanup task, unit tests, and a small benchmark or demo that hammers it from many tasks.
- **Out for now:** distributed state, persistence, hot config reload, an HTTP server layer.

## Easy talking points for interviews
- Tradeoff: more shards means less contention but more memory and a coarser cleanup sweep.
- Lazy refill avoids per-bucket timers, so cost scales with requests, not with the number of keys.
- A natural next step is a small HTTP middleware (axum or tower) that calls `check()`.

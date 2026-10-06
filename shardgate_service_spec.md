# ShardGate: service wrapper and deployment spec

**Goal:** show that ShardGate works as a networked service under load, that anyone can run it in one command, and that you have measured numbers to back it up. Everything below is something an interviewer can verify by cloning the repo.

**Rule for the resume:** only write a bullet once the matching item is committed and the number is in the README.

## Priority order (if time runs short, stop after item 4)

### 1. Single-mutex baseline benchmark (about 1 hour)
- Add a naive limiter (one `Mutex<HashMap<String, Bucket>>`) behind the same `check(key)` interface.
- Benchmark both at 1, 2, 4, and 8 threads, for hot key and spread keys.
- Put a results table in the README.
- **Done when:** the table is committed. If sharding does not win at 1 thread, say so. That is expected and shows you understand the tradeoff.

### 2. HTTP service (about 2 hours)
- New binary (axum on Tokio), e.g. `src/bin/shardgate-server.rs`.
- `GET /check?key=<k>`: 200 if allowed, 429 if denied with a `Retry-After` header (whole seconds, rounded up) and a small JSON body.
- `GET /healthz`: returns 200.
- Config through env vars or flags: capacity, refill rate, shard count, port.
- Graceful shutdown on SIGTERM, and the sweeper started at boot.
- **Done when:** `curl` shows 200s then a 429 with `Retry-After` on a hot key.

### 3. Metrics (about 30 minutes)
- `GET /metrics` in plain text: `allowed_total`, `denied_total`, `active_buckets`.
- Use atomic counters. Do not log per request, since logging would dominate the load test. Log startup and sweeper evictions only.

### 4. Load test (about 1 hour)
- Release build, `wrk` with a small Lua script that randomizes the key (spread) and a run with one fixed key (hot).
- 30 seconds each, e.g. 64 connections. Record req/s, p50, p99, and the allowed/denied split from `/metrics`.
- Write down the machine, and that client and server share it, so the numbers are conservative and noisy.
- **Done when:** a README table has hot and spread results.

### 5. Containerization and CI (about 1 hour)
- Multi-stage Dockerfile: Rust builder, slim runtime image, non-root user, `EXPOSE`, a `HEALTHCHECK` hitting `/healthz`. Record the final image size.
- GitHub Actions on every push: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `docker build`.
- **Done when:** `docker run -p 8080:8080 ...` works from a fresh clone and the CI badge is green.

### 6. Repo hygiene (about 20 minutes)
- Fill in the About description and topics (`rust`, `rate-limiter`, `tokio`, `concurrency`).
- README sections: Architecture, Service API, Benchmarks (baseline table), Load test, Run with Docker, Limitations.
- Limitations to state honestly: single node, in-memory, per-process limits (not global), no persistence.

## Wording note
Say "containerized" unless you actually deploy it somewhere (a cloud host, a VM). A Docker image you run locally is not a deployment, and an interviewer will ask.

## Questions to be ready for
- Why 16 shards, and how would you choose the number?
- What happens when one key is very hot? (One shard's lock becomes the bottleneck.)
- Why a monotonic clock for refill, and how do you test refill without sleeping?
- How does memory stay bounded? (The sweeper. What if it falls behind?)
- Limits are per process. How would you make them global across many instances? (Consistent-hash routing by key, or a shared store, and what each costs.)

## Resume bullets to paste once done
Replace the Tokio demo bullet and add one more, filling in the brackets from your README:

- Benchmarked against a single-mutex baseline ([N]x higher throughput at 8 threads) and load tested as an axum/Tokio HTTP service at [X] req/s with p99 [Y] ms, returning 429 with `Retry-After` on denial.
- Containerized with a multi-stage Docker build ([N] MB image) and GitHub Actions CI running fmt, clippy, tests, and the image build on every push; exposes `/metrics` for allowed/denied counts and live bucket count.

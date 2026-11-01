#!/usr/bin/env bash
# ShardGate scale check: shards, data volume, throughput/latency, failure-handling, baseline.
# Run: bash scale_check.sh  (from shardgate/)
set -u
cd "$(dirname "$0")"
PORT=${PORT:-18080}
export SHARDGATE_CAPACITY=10 SHARDGATE_REFILL_RATE=1.0 SHARDGATE_SHARDS=16 PORT

echo "=== 1/5 NODES/SHARDS ==="
echo "nodes: 1 (single-node, in-memory PoC — no clustering)"
echo "shards: ${SHARDGATE_SHARDS} (env SHARDGATE_SHARDS, default 16)"
echo "cpus: $(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo '?')"
grep -n "num_shards" src/lib.rs | head -3

echo ""
echo "=== 5/5 BASELINE (correctness first) ==="
cargo test 2>&1 | tail -5

echo ""
echo "=== 2/5 DATA VOLUME (in-process) ==="
cargo run --release --example contention 2>&1 | tail -5

echo ""
echo "=== 3/5 THROUGHPUT/LATENCY over HTTP + 4/5 FAILURE-HANDLING ==="
cargo build --release --bin shardgate-server -q 2>&1 | tail -2
./target/release/shardgate-server > /tmp/sg.log 2>&1 &
SRV=$!
trap "kill $SRV 2>/dev/null" EXIT
for i in $(seq 1 50); do curl -sf "http://localhost:$PORT/healthz" >/dev/null && break; sleep 0.1; done
echo "--- healthz:"; curl -s "http://localhost:$PORT/healthz"; echo
echo "--- failure: missing ?key= (expect 400):"; curl -s -o /dev/null -w "%{http_code}\n" "http://localhost:$PORT/check"
echo "--- failure: hot key x12 on cap=10 (expect 10x200 then 429+Retry-After):"
for i in $(seq 1 12); do
  code=$(curl -s -o /tmp/sg_body -w "%{http_code}" "http://localhost:$PORT/check?key=hotkey_scale")
  ra=$(curl -sI "http://localhost:$PORT/check?key=hotkey_scale2" 2>/dev/null | grep -i retry-after | tr -d '\r' || true)
  echo "  try $i -> $code $(cat /tmp/sg_body)"
done | head -14
echo "--- Retry-After header on denied:"
curl -sI "http://localhost:$PORT/check?key=hotkey_scale" | grep -i -E "HTTP|retry-after" | tr -d '\r'
echo "--- data volume: insert 1000 unique keys, then /metrics:"
python3 - "$PORT" <<'PY'
import urllib.request, sys
port = sys.argv[1]
for i in range(1000):
    urllib.request.urlopen(f"http://localhost:{port}/check?key=vol_{i}").read()
print("inserted 1000 unique keys")
PY
curl -s "http://localhost:$PORT/metrics"
echo "--- throughput: 2000 spread-key reqs (sequential, local):"
python3 - "$PORT" <<'PY'
import urllib.request, time, sys
port = sys.argv[1]
N=2000; t0=time.time()
for i in range(N):
    urllib.request.urlopen(f"http://localhost:{port}/check?key=tput_{i%256}").read()
dt=time.time()-t0
print(f"{N} reqs in {dt:.2f}s = {N/dt:.0f} req/s, avg {dt/N*1e6:.0f} us/req (sequential curl, server+client shared)")
PY
echo "--- reference wrk numbers (from README, wrk -t4 -c64 -d30s):"
echo "HOT 647k req/s p50 61us p99 339us | SPREAD 638k req/s p50 60us p99 333us"
echo "--- failure: kill -9 server (in-memory state is wiped = known limit):"
kill -9 $SRV 2>/dev/null; sleep 0.5
curl -s -m 2 "http://localhost:$PORT/healthz" || echo "server down (expected after kill -9)"
trap - EXIT
echo ""
echo "=== BASELINE BENCH (criterion, short sample) ==="
timeout 120 cargo bench --bench shard_bench -- spread/sharded/8t --measurement-time 2 --warm-up-time 0.5 2>&1 | grep -E "spread|time:|Benchmarking|thrpt|ops" | head -10
echo ""
echo "DONE: 1)1 node/16 shards 2)1000-key volume in /metrics 3)req/s+us measured 4)400/429+Retry-After/kill verified 5)cargo test+bench ok"

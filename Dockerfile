# ShardGate lunchbox: cook in Rust, serve slim.
# Build: docker build -t shardgate .
# Run: docker run -p 8080:8080 -e SHARDGATE_CAPACITY=10 shardgate

FROM rust:1-bookworm AS builder
WORKDIR /app
# Copy full context (respects .dockerignore) — needs src + benches for manifest.
COPY . ./
# Build only the server (lib test still runs in CI).
RUN cargo build --release --bin shardgate-server

FROM debian:bookworm-slim
# curl for HEALTHCHECK. Clean apt lists to keep image small.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
# Non-root user (never run internet-facing as root).
RUN useradd -m appuser
WORKDIR /app
COPY --from=builder /app/target/release/shardgate-server ./shardgate-server
USER appuser
EXPOSE 8080
# Docker pings the back door; unhealthy if no 200.
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/healthz || exit 1
CMD ["./shardgate-server"]

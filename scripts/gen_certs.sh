#!/usr/bin/env bash
# Self-signed cert for ShardGate TLS (test only, 30 days).
set -euo pipefail
OUT="${1:-certs}"
mkdir -p "$OUT"
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$OUT/key.pem" -out "$OUT/cert.pem" \
  -days 30 -subj "/CN=localhost" \
  -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" 2>/dev/null
echo "wrote $OUT/cert.pem $OUT/key.pem"

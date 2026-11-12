#!/usr/bin/env bash
# check-ship.sh — the pack-side ship gate (DESIGN.md §10, §13):
#   * dist/harness.html size ≤ 2.5 MB
#   * every <script> block passes node --check
#   * single file: no required CDN, no sibling asset fetch
#   * iPhone Safari CSS gates (16px inputs, 44px targets, dvh+vh, safe-area,
#     visualViewport var)
#   * secret-grep clean
# Exits non-zero on any failure.

set -eu
cd "$(dirname "$0")/.."

echo "== proxy + core tests =="
cargo test --workspace

echo "== pack checks (size, node --check, single-file, CSS gates) =="
node --test tests/pack.test.mjs

echo "== secret grep =="
scripts/secret-grep.sh

echo "== size =="
SIZE=$(wc -c < dist/harness.html)
echo "dist/harness.html: ${SIZE} bytes (limit $((2500 * 1024)))"
if [ "$SIZE" -gt $((2500 * 1024)) ]; then
    echo "SIZE GATE FAILED"
    exit 1
fi

echo "check-ship: PASS"

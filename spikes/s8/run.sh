#!/usr/bin/env bash
# S8 — reproduce every number in README.md, in the order it reports them.
set -euo pipefail
cd "$(dirname "$0")"

cargo build --release --bins

echo "== contract, and the bytecode round-trip"
./target/release/s8-quickjs

echo
echo "== engine-only size"
floor=$(stat -c %s target/release/floor)
gzipped=$(gzip -9 -c target/release/floor | wc -c)
printf '  QuickJS-ng floor  %s B installed, %s B gzip -9\n' "$floor" "$gzipped"
printf '  JavaScriptCore    37980984 B installed, 17933416 B gzip -9  (spikes/s0)\n'

echo
echo "== engine init"
./target/release/init

echo
echo "== synthetic throughput"
./target/release/compare

echo
echo "== pong's own frame"
./target/release/frame

echo
echo "== crossing cost against work cost"
./target/release/crossover

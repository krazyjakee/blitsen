#!/usr/bin/env bash
# S8 — reproduce the QuickJS-ng numbers in README.md, in the order it reports
# them. The JavaScriptCore arms this spike measured against are gone with
# `crates/blitsen-jsc`; see "Reproduce" in README.md.
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

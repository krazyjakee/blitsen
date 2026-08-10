#!/usr/bin/env bash
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
mkdir -p "$here/results"

cc -shared -fPIC -I/usr/include/node "$here/option2.c" -o "$here/option2.node"
set +e
timeout 2s bun "$here/option2.mjs" > "$here/results/option2.txt" 2>&1
option2_status=$?
set -e
printf 'exit_status=%s\n' "$option2_status" >> "$here/results/option2.txt"

cargo build --manifest-path "$here/Cargo.toml" --release
cp "$here/target/release/libs1_winit_pump.so" "$here/s1_winit_pump.node"
S1_BUSY_MS=0 bun "$here/fallback.mjs" | tee "$here/results/fallback-idle-linux-x64.json"
S1_BUSY_MS=4 bun "$here/fallback.mjs" | tee "$here/results/fallback-4ms-linux-x64.json"

cat "$here/results/option2.txt"
if [[ "$option2_status" -eq 0 ]]; then
  echo "Option 2 unexpectedly returned; inspect the result before drawing a conclusion." >&2
fi

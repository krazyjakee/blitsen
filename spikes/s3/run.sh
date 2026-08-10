#!/usr/bin/env bash
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
measure="$here/.measure"
mkdir -p "$measure"

cargo build --manifest-path "$here/Cargo.toml" --release
cp "$here/target/release/libs3_window.so" "$here/s3_window.node"

bun "$here/main.js" | tee "$measure/direct.json"

bun build --target=bun --compile "$here/main.js" --outfile "$measure/s3-compiled"
temporary_dir=$(mktemp -d)
trap 'rm -rf -- "$temporary_dir"' EXIT
(
  cd "$temporary_dir"
  "$measure/s3-compiled"
) | tee "$measure/compiled.json"

printf 'addon_bytes\t%s\n' "$(stat -c '%s' "$here/s3_window.node")"
printf 'compiled_bytes\t%s\n' "$(stat -c '%s' "$measure/s3-compiled")"

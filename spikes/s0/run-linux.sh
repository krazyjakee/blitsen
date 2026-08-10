#!/usr/bin/env bash
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
cache=${BLITSEN_S0_CACHE:-"$here/.cache"}
measure=${BLITSEN_S0_MEASURE:-"$here/.measure"}
webkit_revision=447082ab6897278727b44e1ba3c326ae6e1504c3
mimalloc_revision=1803341d6241d8fa4b3f65fa68cb13a32ad92f04
release_url="https://github.com/oven-sh/WebKit/releases/download/autobuild-$webkit_revision"

mkdir -p "$cache" "$measure"

fetch_webkit() {
  local flavour=$1
  local archive="bun-webkit-linux-amd64${flavour}.tar.gz"
  local destination="$cache/webkit${flavour}"
  if [[ ! -d "$destination/bun-webkit/lib" ]]; then
    curl --fail --location --retry 3 --output "$cache/$archive" "$release_url/$archive"
    mkdir -p "$destination"
    tar -xzf "$cache/$archive" -C "$destination"
  fi
}

fetch_webkit ""
fetch_webkit "-lto"

if [[ ! -d "$cache/mimalloc/.git" ]]; then
  git clone https://github.com/oven-sh/mimalloc.git "$cache/mimalloc"
fi
git -C "$cache/mimalloc" fetch --depth 1 origin "$mimalloc_revision"
git -C "$cache/mimalloc" checkout --detach "$mimalloc_revision"

export BLITSEN_MIMALLOC_DIR="$cache/mimalloc"
export BLITSEN_JSC_LIB_DIR="$cache/webkit/bun-webkit/lib"
cargo build --manifest-path "$here/Cargo.toml"
cargo build --manifest-path "$here/Cargo.toml" --release

cp "$here/target/debug/s0-jsc-blitz-size" "$measure/debug"
cp "$here/target/release/s0-jsc-blitz-size" "$measure/release"
cp "$measure/release" "$measure/release-strip"
strip "$measure/release-strip"

export BLITSEN_JSC_LIB_DIR="$cache/webkit-lto/bun-webkit/lib"
CARGO_TARGET_DIR="$cache/target-production-lto" \
  cargo build --manifest-path "$here/Cargo.toml" --profile production
cp "$cache/target-production-lto/production/s0-jsc-blitz-size" "$measure/release-strip-lto"

export BLITSEN_JSC_LIB_DIR="$cache/webkit/bun-webkit/lib"
CARGO_TARGET_DIR="$cache/target-jsc" \
  cargo build --manifest-path "$here/Cargo.toml" --release --no-default-features --features jsc
CARGO_TARGET_DIR="$cache/target-blitz" \
  cargo build --manifest-path "$here/Cargo.toml" --release --no-default-features --features blitz
cp "$cache/target-jsc/release/s0-jsc-blitz-size" "$measure/jsc-only"
cp "$cache/target-blitz/release/s0-jsc-blitz-size" "$measure/blitz-only"
strip "$measure/jsc-only" "$measure/blitz-only"

printf 'variant\tinstalled_bytes\tgzip_9_bytes\n' > "$measure/linux-x64.tsv"
for variant in debug release release-strip release-strip-lto jsc-only blitz-only; do
  gzip -9 -c "$measure/$variant" > "$measure/$variant.gz"
  printf '%s\t%s\t%s\n' \
    "$variant" \
    "$(stat -c '%s' "$measure/$variant")" \
    "$(stat -c '%s' "$measure/$variant.gz")" >> "$measure/linux-x64.tsv"
  "$measure/$variant"
done
cat "$measure/linux-x64.tsv"

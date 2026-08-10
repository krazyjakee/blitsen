#!/usr/bin/env bash
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
cargo build --manifest-path "$here/Cargo.toml" --release
"$here/target/release/s5-wgpu-composite"

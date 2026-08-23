#!/usr/bin/env bash
# The one release build invocation, including the few flags that define its
# reproducibility boundary. Called from release.yml for the shipping build and
# again from a second clean checkout for the selected comparison targets (#71).
set -euo pipefail

target=${1:?usage: build-release-runtime.sh <blitsen-target>}

# Rust can embed the checkout path through file!/panic diagnostics even after
# symbols are stripped. Both clean checkouts map to the same logical source
# root so a runner's temporary path is not part of the artifact. Git Bash gives
# native tools Windows paths through `pwd -W`; other shells use POSIX paths.
case "${OSTYPE:-}" in
  msys* | cygwin*) source_root=$(pwd -W) ;;
  *) source_root=$PWD ;;
esac
release_flags="--remap-path-prefix=${source_root}=/src/blitsen"

# QuickJS-ng and a few platform dependencies compile C/C++ through the `cc`
# crate. rustc's remap does not reach their __FILE__ strings, so give the pinned
# native compiler the equivalent mapping as well.
case "$target" in
  win32-*) native_map="/pathmap:${source_root}=/src/blitsen" ;;
  *) native_map="-ffile-prefix-map=${source_root}=/src/blitsen" ;;
esac
export CFLAGS="${CFLAGS:+$CFLAGS }$native_map"
export CXXFLAGS="${CXXFLAGS:+$CXXFLAGS }$native_map"

# link.exe otherwise owns the PE timestamp. /Brepro asks the pinned MSVC linker
# for deterministic metadata as well as deterministic section contents.
case "$target" in
  win32-*) release_flags="$release_flags -C target-feature=+crt-static -C link-arg=/Brepro" ;;
esac

# SOURCE_DATE_EPOCH is the source commit, not the time this particular runner
# happened to start. Cargo/rustc do not promise to consume it themselves, but
# native build scripts which follow the standard receive a stable value.
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git show -s --format=%ct HEAD)}
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }$release_flags"

cargo build --locked --release -p blitsen-node -p blitsen-runtime

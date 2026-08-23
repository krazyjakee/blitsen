#!/usr/bin/env bash
# The one release build invocation, including the few flags that define its
# reproducibility boundary. Called from release.yml for the shipping build and
# again from a second clean checkout for the selected comparison targets (#71).
set -euo pipefail

target=${1:?usage: build-release-runtime.sh <blitsen-target>}

# npm is the native distribution boundary. A release helper invocation always
# stamps that package version, while an ordinary `cargo build` deliberately
# remains an unversioned checkout build. Validate all seven manifests here as
# well as in the workflow so a local release build cannot create a falsely
# labelled artifact.
manifest_version=$(bun scripts/release-version.mjs manifests \
  packages/blitsen/package.json packages/platforms/*/package.json)
if [[ -n "${BLITSEN_RELEASE_VERSION:-}" && "$BLITSEN_RELEASE_VERSION" != "$manifest_version" ]]; then
  echo "BLITSEN_RELEASE_VERSION $BLITSEN_RELEASE_VERSION disagrees with npm manifests $manifest_version" >&2
  exit 1
fi
export BLITSEN_RELEASE_VERSION=$manifest_version

# Rust can embed the checkout path through file!/panic diagnostics even after
# symbols are stripped. Both clean checkouts map to the same logical source
# root so a runner's temporary path is not part of the artifact. Git Bash gives
# native tools Windows paths through `pwd -W`; other shells use POSIX paths.
case "${OSTYPE:-}" in
  msys* | cygwin*) source_root=$(pwd -W) ;;
  *) source_root=$PWD ;;
esac

# CARGO_ENCODED_RUSTFLAGS keeps each flag byte-for-byte. This matters on
# Windows: RUSTFLAGS is whitespace-split and a backslash spelling of a path can
# otherwise be consumed as shell escaping before it reaches rustc.
encoded_rustflags=${CARGO_ENCODED_RUSTFLAGS:-}
if [[ -z "$encoded_rustflags" && -n "${RUSTFLAGS:-}" ]]; then
  read -r -a inherited_rustflags <<< "$RUSTFLAGS"
  for flag in "${inherited_rustflags[@]}"; do
    encoded_rustflags="${encoded_rustflags:+${encoded_rustflags}$'\x1f'}$flag"
  done
fi
unset RUSTFLAGS
append_rustflag() {
  encoded_rustflags="${encoded_rustflags:+${encoded_rustflags}$'\x1f'}$1"
}
append_rustflag "--remap-path-prefix=${source_root}=/src/blitsen"

# QuickJS-ng and a few platform dependencies compile C/C++ through the `cc`
# crate. rustc's remap does not reach their __FILE__ strings, so give the pinned
# native compiler the equivalent mapping as well.
case "$target" in
  win32-*)
    # rustc and cl.exe can receive the same absolute path with either separator.
    # Prefix remapping is deliberately textual, so cover both representations.
    source_root_backslash=${source_root//\//\\}
    append_rustflag "--remap-path-prefix=${source_root_backslash}=/src/blitsen"
    native_map="/pathmap:${source_root}=/src/blitsen /pathmap:${source_root_backslash}=/src/blitsen"
    ;;
  *) native_map="-ffile-prefix-map=${source_root}=/src/blitsen" ;;
esac
export CFLAGS="${CFLAGS:+$CFLAGS }$native_map"
export CXXFLAGS="${CXXFLAGS:+$CXXFLAGS }$native_map"

# link.exe otherwise owns the PE timestamp. /Brepro asks the pinned MSVC linker
# for deterministic metadata as well as deterministic section contents.
case "$target" in
  win32-*)
    append_rustflag "-C"
    append_rustflag "target-feature=+crt-static"
    append_rustflag "-C"
    append_rustflag "link-arg=/Brepro"
    # MSVC otherwise records the absolute PDB location in the PE image. The
    # PDB itself is not shipped, and its basename is stable across the two
    # builds, so keep the useful debugger hint without retaining either
    # checkout root.
    append_rustflag "-C"
    append_rustflag 'link-arg=/PDBALTPATH:%_PDB%'
    ;;
esac

# SOURCE_DATE_EPOCH is the source commit, not the time this particular runner
# happened to start. Cargo/rustc do not promise to consume it themselves, but
# native build scripts which follow the standard receive a stable value.
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git show -s --format=%ct HEAD)}
export CARGO_ENCODED_RUSTFLAGS=$encoded_rustflags

cargo build --locked --release -p blitsen-node -p blitsen-runtime

#!/bin/bash
set -eu

# Deliberately bash and not `/bin/sh`. Cargo names an integration test's sibling
# executables with the bin target spelled verbatim, so `blitsen-runtime` arrives
# as `CARGO_BIN_EXE_blitsen-runtime` — a hyphen, which is not a valid shell
# identifier. Dash, which is `/bin/sh` on Debian and Ubuntu, drops every such
# variable out of the environment it passes on, and it does so before this file
# runs, so nothing here can put them back. `env!("CARGO_BIN_EXE_blitsen-runtime")`
# then fails to compile in `crates/blitsen-runtime/tests` for the one reason no
# error message names: a wrapper stood between Cargo and rustc. Bash keeps the
# variables, so the exec below is transparent, which is the only thing a
# compiler wrapper is allowed to be. Windows never reaches this file; CI clears
# `RUSTC_WRAPPER` there because a POSIX script is not executable on that runner.

if [ "${BLITSEN_DISABLE_SCCACHE:-0}" != "1" ] && command -v sccache >/dev/null 2>&1; then
    exec sccache "$@"
fi

exec "$@"

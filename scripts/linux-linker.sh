#!/bin/sh
set -eu

if command -v mold >/dev/null 2>&1; then
    exec cc -fuse-ld=mold "$@"
fi

exec cc "$@"

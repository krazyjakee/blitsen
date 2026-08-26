#!/bin/sh
set -eu

if [ "${BLITSEN_DISABLE_SCCACHE:-0}" != "1" ] && command -v sccache >/dev/null 2>&1; then
    exec sccache "$@"
fi

exec "$@"

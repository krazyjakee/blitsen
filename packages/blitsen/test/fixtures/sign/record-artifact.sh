#!/bin/sh
# Stands in for codesign/signtool: records the artifact the hook was handed.
printf '%s\n' "$1" > "$1.signed"

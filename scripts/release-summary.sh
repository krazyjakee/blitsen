#!/usr/bin/env bash
set -euo pipefail

validate_tag() {
  local tag=$1
  if [[ ! $tag =~ ^[A-Za-z][A-Za-z0-9._-]{0,127}$ || $tag =~ ^[vV][0-9] ]]; then
    printf 'Invalid npm dist-tag: %q\n' "$tag" >&2
    return 1
  fi
}

case "${1:-}" in
  validate-tag)
    validate_tag "${2:-}"
    ;;
  summary)
    : "${GITHUB_STEP_SUMMARY:?GITHUB_STEP_SUMMARY is required}"
    : "${DIST_TAG:?DIST_TAG is required}"
    : "${PUBLISHED:?PUBLISHED is required}"
    : "${VERSION:?VERSION is required}"
    validate_tag "$DIST_TAG"
    {
      printf '## Release %s\n\n' "$VERSION"
      printf 'Published: %s (tag `%s`)\n' "$PUBLISHED" "$DIST_TAG"
      if [[ -n ${RELEASE_URL:-} ]]; then
        printf '\nTagged `v%s` — %s\n' "$VERSION" "$RELEASE_URL"
      fi
      printf '\nNotarisation is **not** part of this workflow. Submit the macOS artifacts\n'
      printf 'separately with `notarytool`; see docs/RELEASING.md.\n'
    } >> "$GITHUB_STEP_SUMMARY"
    ;;
  *)
    printf 'usage: %s {validate-tag TAG|summary}\n' "$0" >&2
    exit 2
    ;;
esac

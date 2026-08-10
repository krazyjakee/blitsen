#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
OUTPUT_DIR="$SCRIPT_DIR/results"
WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/blitsen-s6.XXXXXX")

BLITZ_REV=1efe22d2524d71ede5b94592204c21f0de644219
REACT_REV=70cfd3098f219f09a3c6941b2d1fabe4665dfa3d
VUE_REV=a3b07312d4c416c3976a3012e64cf39053060708
SVELTE_REV=199122be1f3ed71f5cf4abd5748debd91ee540a0

server_pids=()
cleanup() {
  if ((${#server_pids[@]})); then
    kill "${server_pids[@]}" 2>/dev/null || true
  fi
}
trap cleanup EXIT

for command in bun cargo chromium compare convert corepack git npm python3; do
  command -v "$command" >/dev/null || {
    printf 'missing required command: %s\n' "$command" >&2
    exit 1
  }
done

checkout_revision() {
  local repository=$1
  local revision=$2
  local destination=$3
  git init -q "$destination"
  git -C "$destination" remote add origin "$repository"
  git -C "$destination" fetch -q --depth 1 origin "$revision"
  git -C "$destination" checkout -q --detach FETCH_HEAD
}

checkout_revision https://github.com/DioxusLabs/blitz.git "$BLITZ_REV" "$WORK_DIR/blitz"
checkout_revision https://github.com/satnaing/shadcn-admin.git "$REACT_REV" "$WORK_DIR/react"
checkout_revision https://github.com/mutoe/vue3-realworld-example-app.git "$VUE_REV" "$WORK_DIR/vue"
checkout_revision https://github.com/MikhaD/wordle.git "$SVELTE_REV" "$WORK_DIR/svelte"

(
  cd "$WORK_DIR/react"
  corepack pnpm install --frozen-lockfile --config.dangerously-allow-all-builds=true
  corepack pnpm build
)
(
  cd "$WORK_DIR/vue"
  corepack pnpm install --frozen-lockfile
  corepack pnpm build
)
(
  cd "$WORK_DIR/svelte"
  npm install
  npm run build
  ln -s dist wordle
)

mkdir -p \
  "$OUTPUT_DIR/chromium" \
  "$OUTPUT_DIR/blitz-raw" \
  "$OUTPUT_DIR/blitz-snapshot" \
  "$OUTPUT_DIR/diff"

python3 -m http.server 48101 --bind 127.0.0.1 --directory "$WORK_DIR/react/dist" &
server_pids+=("$!")
python3 -m http.server 48102 --bind 127.0.0.1 --directory "$WORK_DIR/vue/dist" &
server_pids+=("$!")
python3 -m http.server 48104 --bind 127.0.0.1 --directory "$WORK_DIR/svelte" &
server_pids+=("$!")

S6_OUTPUT_DIR="$OUTPUT_DIR" \
S6_CHROMIUM_PROFILE="$WORK_DIR/chromium-profile" \
S6_REACT_DIST="$WORK_DIR/react/dist" \
S6_VUE_DIST="$WORK_DIR/vue/dist" \
S6_SVELTE_DIST="$WORK_DIR/svelte/dist" \
  bun "$SCRIPT_DIR/capture.mjs"

cargo build --release --example screenshot --manifest-path "$WORK_DIR/blitz/Cargo.toml"
BLITZ_SCREENSHOT="$WORK_DIR/blitz/target/release/examples/screenshot"

render_blitz() {
  local url=$1
  local destination=$2
  local log source
  log=$($BLITZ_SCREENSHOT "$url" 1440)
  printf '%s\n' "$log"
  source=$(awk '/Written to / { print $3 }' <<<"$log")
  cp "$source" "$destination"
}

render_blitz http://127.0.0.1:48101/ "$OUTPUT_DIR/blitz-raw/react.png"
render_blitz http://127.0.0.1:48102/ "$OUTPUT_DIR/blitz-raw/vue.png"
render_blitz http://127.0.0.1:48104/wordle/ "$OUTPUT_DIR/blitz-raw/svelte.png"
render_blitz http://127.0.0.1:48101/snapshot.html "$OUTPUT_DIR/blitz-snapshot/react.png"
render_blitz http://127.0.0.1:48102/snapshot.html "$OUTPUT_DIR/blitz-snapshot/vue.png"
render_blitz http://127.0.0.1:48104/wordle/snapshot.html "$OUTPUT_DIR/blitz-snapshot/svelte.png"

convert "$OUTPUT_DIR/blitz-snapshot/vue.png" \
  -crop 2880x1600+0+0 +repage "$WORK_DIR/vue-crop.png"

{
  printf 'app\trmse\tnormalized_rmse\tncc\n'
  for app in react vue svelte; do
    candidate="$OUTPUT_DIR/blitz-snapshot/$app.png"
    [[ "$app" == vue ]] && candidate="$WORK_DIR/vue-crop.png"
    rmse=$(compare -metric RMSE "$OUTPUT_DIR/chromium/$app.png" "$candidate" \
      "$OUTPUT_DIR/diff/$app.png" 2>&1 || true)
    ncc=$(compare -metric NCC "$OUTPUT_DIR/chromium/$app.png" "$candidate" \
      null: 2>&1 || true)
    absolute=${rmse%% *}
    normalized=${rmse##*\(}
    normalized=${normalized%\)}
    printf '%s\t%s\t%s\t%s\n' "$app" "$absolute" "$normalized" "$ncc"
  done
} >"$OUTPUT_DIR/metrics.tsv"

printf 'S6 artifacts written to %s\n' "$OUTPUT_DIR"
printf 'Temporary sources retained at %s\n' "$WORK_DIR"

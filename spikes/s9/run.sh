#!/usr/bin/env bash
# S9 -- link crates/blitsen-android into an APK, run it on an emulator, and look
# at the pixels.
#
#   ./run.sh                              the document
#   ./run.sh --control background         the same document with two colours changed
#   ./run.sh --control no-entrypoint      the same APK with index.html left out
#   ./run.sh --control config-declared    a configuration change, manifest claims it
#   ./run.sh --control config-undeclared  a configuration change, manifest does not
#
# Everything lands in results/. The emulator must already be running with
# `-gpu host`; see README.md, and note that `-gpu swiftshader_indirect` kills it
# the moment wgpu initialises (#139).
set -euo pipefail
cd "$(dirname "$0")"
SPIKE=$PWD
ROOT=$(cd ../.. && pwd)

: "${ANDROID_HOME:=$HOME/Android/Sdk}"
: "${ANDROID_NDK_HOME:=$ANDROID_HOME/ndk/27.2.12479018}"
# rquickjs-sys ships no Android bindings and generates them with bindgen, so a
# libclang is a build-time requirement for this target and only this target.
: "${LIBCLANG_PATH:=/usr/lib/llvm-18/lib}"
export ANDROID_HOME ANDROID_NDK_HOME LIBCLANG_PATH

BUILD_TOOLS=${BLITSEN_S9_BUILD_TOOLS:-$ANDROID_HOME/build-tools/34.0.0}
PLATFORM=${BLITSEN_S9_PLATFORM:-$ANDROID_HOME/platforms/android-33/android.jar}
SERIAL=${BLITSEN_S9_SERIAL:-emulator-5556}
KEYSTORE=${BLITSEN_S9_KEYSTORE:-$HOME/.android/debug.keystore}
ABI=x86_64
TRIPLE=x86_64-linux-android
PACKAGE=dev.blitsen.s9

CONTROL=none
while (($#)); do
  case $1 in
    --control) CONTROL=$2; shift 2 ;;
    *) printf 'unknown option %s\n' "$1" >&2; exit 2 ;;
  esac
done

for tool in cargo cargo-ndk adb python3 zip; do
  command -v "$tool" >/dev/null || { printf 'missing %s\n' "$tool" >&2; exit 1; }
done
for tool in aapt2 zipalign apksigner; do
  [[ -x $BUILD_TOOLS/$tool ]] || { printf 'missing %s/%s\n' "$BUILD_TOOLS" "$tool" >&2; exit 1; }
done

WORK=$(mktemp -d "${TMPDIR:-/tmp}/blitsen-s9.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
mkdir -p results

case $CONTROL in
  none)         NAME=frame;             PAGE=0b1020; JS=ff00aa ;;
  background)   NAME=control-background; PAGE=3b0b52; JS=00ffd0 ;;
  no-entrypoint) NAME=control-no-entrypoint; PAGE=0b1020; JS=ff00aa ;;
  config-declared)   NAME=control-config-declared;   PAGE=0b1020; JS=ff00aa ;;
  config-undeclared) NAME=control-config-undeclared; PAGE=0b1020; JS=ff00aa ;;
  *) printf 'unknown control %s\n' "$CONTROL" >&2; exit 2 ;;
esac

echo "== 1. cross-compile crates/blitsen-android for $TRIPLE"
(cd "$ROOT" && cargo ndk -t "$ABI" -P 33 build --release -p blitsen-android)
SO=$ROOT/target/$TRIPLE/release/libblitsen_android.so
[[ -f $SO ]] || { printf 'no %s\n' "$SO" >&2; exit 1; }
printf '   %s  %s bytes\n' "$(basename "$SO")" "$(stat -c %s "$SO")"

echo "== 2. stage the application"
STAGE=$WORK/stage
mkdir -p "$STAGE/lib/$ABI" "$STAGE/assets/blitsen"
"$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip" \
  -o "$STAGE/lib/$ABI/libblitsen_android.so" "$SO"
cp app/index.html app/style.css app/app.js app/blitsen.runtime.json "$STAGE/assets/blitsen/"

case $CONTROL in
  background)
    # One document, two literals moved. If the captured frame does not follow,
    # the frame was never this document.
    sed -i "s/background: #0b1020;/background: #$PAGE;/" "$STAGE/assets/blitsen/style.css"
    sed -i "s/rgb(255, 0, 170)/rgb(0, 255, 208)/" "$STAGE/assets/blitsen/app.js"
    ;;
  no-entrypoint)
    # The APK is otherwise identical, so what comes back distinguishes "Blitsen
    # looked and said so" from "nothing ran".
    rm "$STAGE/assets/blitsen/index.html"
    ;;
esac

# blitsen.assets.json, written by hand because no packaging step writes it yet
# (that is #148). Format from crates/blitsen-host/src/apk.rs: a version and a
# list of application-relative paths with their lengths.
python3 - "$STAGE/assets/blitsen" <<'PY'
import json, os, sys
root = sys.argv[1]
files = [
    {"path": name, "bytes": os.path.getsize(os.path.join(root, name))}
    for name in sorted(os.listdir(root))
]
with open(os.path.join(root, "blitsen.assets.json"), "w") as handle:
    json.dump({"version": 1, "files": files}, handle, indent=2)
    handle.write("\n")
PY
find "$STAGE/assets" -type f -printf '   %s\t%P\n' | sort -k2

echo "== 3. link the APK"
MANIFEST=apk/AndroidManifest.xml
if [[ $CONTROL == config-undeclared ]]; then
  # The same manifest with its `configChanges` list taken out. The pair
  # `config-declared` / `config-undeclared` is one variable: the same document,
  # the same `.so`, the same provocation, and only this attribute between them.
  MANIFEST=$WORK/AndroidManifest.undeclared.xml
  sed 's/ android:configChanges="[^"]*"//' apk/AndroidManifest.xml > "$MANIFEST"
  if grep -q 'android:configChanges=' "$MANIFEST"; then
    printf 'the configChanges attribute survived the edit\n' >&2
    exit 1
  fi
fi
"$BUILD_TOOLS/aapt2" link \
  -o "$WORK/unaligned.apk" \
  -I "$PLATFORM" \
  --manifest "$MANIFEST" \
  --min-sdk-version 24 \
  --target-sdk-version 33
# -0: stored, never deflated. The .so because android:extractNativeLibs="false"
# requires it, and assets/ because that is the one packaging instruction #144
# asks for -- an asset Gradle deflates is inflated on every read.
(cd "$STAGE" && zip -q -0 -X -r "$WORK/unaligned.apk" lib assets)
"$BUILD_TOOLS/zipalign" -f -p 4 "$WORK/unaligned.apk" "$WORK/s9.apk"
"$BUILD_TOOLS/apksigner" sign \
  --ks "$KEYSTORE" --ks-pass pass:android \
  --ks-key-alias androiddebugkey --key-pass pass:android \
  --min-sdk-version 24 \
  "$WORK/s9.apk"
printf '   s9.apk  %s bytes\n' "$(stat -c %s "$WORK/s9.apk")"

echo "== 4. install on $SERIAL"
adb -s "$SERIAL" wait-for-device
adb -s "$SERIAL" install -r "$WORK/s9.apk"

echo "== 5. launch, with logcat running"
adb -s "$SERIAL" shell am force-stop "$PACKAGE" || true
adb -s "$SERIAL" logcat -c
adb -s "$SERIAL" logcat -v time > "results/$NAME.logcat" &
LOGCAT=$!
trap 'kill $LOGCAT 2>/dev/null || true; rm -rf "$WORK"' EXIT
adb -s "$SERIAL" shell am start -n "$PACKAGE/android.app.NativeActivity"

# Long enough for a cold start plus a first frame, and fixed rather than
# retried: capturing until the picture is the one wanted is not a measurement.
sleep 20

if [[ $CONTROL == config-* ]]; then
  echo "== 5b. change the device configuration under the running application"
  # What #142 predicted here is a panic: `set_android_app` is an upstream
  # `OnceLock::set(..).unwrap()`, so an `android_main` entered a second time in
  # one process cannot survive. Dark mode is used rather than rotation because
  # it is one command, needs no sensor, and is not input of any kind.
  adb -s "$SERIAL" shell cmd uimode night yes
  sleep 10
  adb -s "$SERIAL" shell cmd uimode night no || true
  sleep 5
fi

echo "== 6. capture, twice, five seconds apart"
adb -s "$SERIAL" exec-out screencap -p > "results/$NAME.png"
sleep 5
adb -s "$SERIAL" exec-out screencap -p > "results/$NAME-later.png"
adb -s "$SERIAL" shell pidof "$PACKAGE" > "$WORK/pid" || true
if [[ -s $WORK/pid ]]; then
  printf '   process alive, pid %s\n' "$(cat "$WORK/pid")"
else
  printf '   PROCESS GONE\n'
fi
kill $LOGCAT 2>/dev/null || true
sleep 1
# The full log is enormous and machine-local; what is worth keeping beside the
# frame is the part that names us, plus anything that died.
grep -E 'Blitsen|blitsen|NativeActivity|DEBUG|AndroidRuntime|libc *:' \
  "results/$NAME.logcat" > "results/$NAME.log" || true
cat "results/$NAME.log"

echo "== 7. read the pixels"
if [[ $CONTROL == no-entrypoint || $CONTROL == config-* ]]; then
  # This one is supposed to fail every check; what it is being asked is whether
  # the logcat above says why. Do not let that failure end the script.
  python3 check.py "results/$NAME.png" --page "$PAGE" --js "$JS" \
    --later "results/$NAME-later.png" --json "results/$NAME.json" || true
else
  python3 check.py "results/$NAME.png" --page "$PAGE" --js "$JS" \
    --later "results/$NAME-later.png" --specimens "results/$NAME-specimens.png" \
    --json "results/$NAME.json"
fi

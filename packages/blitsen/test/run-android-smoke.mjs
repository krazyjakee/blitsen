// The Android smoke test: install an APK, launch it, and read the framebuffer
// back (issue #149).
//
//     bun run --cwd packages/blitsen test:android -- \
//       --apk <path> --package com.blitsen.pong
//
// # What this measures, and why it is shaped this way
//
// #139 established the one thing an Android smoke test has to survive: with
// `-gpu swiftshader_indirect` the *emulator itself* died the moment the
// application initialised wgpu, and it did so even for a build that rasterised
// entirely on the CPU and used wgpu only to present. So the failure this is
// most likely to meet is not an assertion going red — it is adb losing the
// device halfway through. Every step below therefore checks that the emulator
// is still answering, and says which of the two happened.
//
// # The controls
//
// A screenshot harness that cannot take a screenshot looks exactly like an
// application that painted nothing, and this repository has a history of green
// checks that measured nothing. So the frame is captured **twice**:
//
//   1. **Before the APK is installed.** That is the launcher, and it must be a
//      normal, varied image. If it is not, the emulator or `screencap` is
//      broken and the run says so and stops — it does not go on to blame
//      Blitsen for a black rectangle.
//   2. **After the application has been on screen for a while.** This one has
//      to be non-blank *and* to differ from the first. The difference is what
//      rules out the case that matters most: `screencap` succeeding while the
//      application never painted, so the launcher is still what is on screen.
//
// Neither check knows anything about Pong. Asserting particular colours would
// measure the example rather than the engine, and #143's spike already did the
// pixel-level work against a document written for it.
//
// # Why raw frames rather than PNG
//
// `adb exec-out screencap -p` emits a PNG and nothing in this package can
// decode one. `adb exec-out screencap` emits the pixels: a small header, then
// width x height 32-bit pixels with the row stride already removed. The header
// grew a colour-space field in Android 9, so it is 12 bytes on older devices
// and 16 on newer ones; rather than assume, [`decodeFrame`] tries both and
// keeps the one whose size arithmetic works out. If neither does, it fails with
// the numbers rather than guessing — a misread header would turn every frame
// into noise, which is to say into a pass.
import { join } from "node:path";

import {
  adb, argument, artifacts, deviceIsAlive, keep as keepIn, sleep, waitForBoot,
} from "./android-device.mjs";

/// A frame is not blank if it has at least this many distinct colours, and if
/// no single colour covers more than this much of it.
///
/// Deliberately weak. A real frame from either the launcher or a Blitsen
/// document has thousands of distinct colours, and the numbers a failure
/// reports are printed either way, so these are a floor under "obviously
/// nothing" rather than a measurement. A tighter bound would start failing on
/// an application that happens to be mostly one colour, which is a thing an
/// application is allowed to be.
const MINIMUM_COLOURS = 32;
const MAXIMUM_UNIFORMITY = 0.995;

/// How much of the frame has to change between the launcher and the running
/// application. Two different screens differ in most of their pixels; this only
/// has to exclude "the same screen with a clock on it".
const MINIMUM_CHANGE = 0.10;

const options = {
  apk: argument("apk"),
  package: argument("package"),
  activity: argument("activity", "android.app.NativeActivity"),
  out: argument("out", join(process.cwd(), "../../target/android-smoke")),
  // How long the application gets to reach a first frame. Generous, because a
  // software-rendered emulator on a hosted runner is slow and a timeout that
  // fires early is indistinguishable from a real failure.
  settle: Number(argument("settle", "45000")),
};

/// The raw `screencap` header, read rather than assumed. See the note above.
///
/// Exported, and the reason is the whole point of this file: if this reads the
/// header wrong every frame becomes noise, noise has plenty of distinct colours,
/// and the smoke test passes without measuring anything. `cli-android.test.mjs`
/// holds it to synthetic frames of both header sizes and to buffers that are not
/// frames at all.
export function decodeFrame(bytes) {
  for (const header of [16, 12]) {
    if (bytes.length < header) continue;
    const width = bytes.readUInt32LE(0);
    const height = bytes.readUInt32LE(4);
    if (width === 0 || height === 0) continue;
    if (bytes.length - header !== width * height * 4) continue;
    const pixels = new Uint32Array(width * height);
    for (let at = 0; at < pixels.length; at += 1) {
      pixels[at] = bytes.readUInt32LE(header + at * 4);
    }
    return { width, height, pixels, header };
  }
  throw new Error(`screencap returned ${bytes.length} bytes that are not a frame this `
    + "understands: neither a 12- nor a 16-byte header leaves width x height x 4 pixels. "
    + "The format changed, and reading it wrong would turn every frame into a pass.");
}

/** One frame off the device, decoded, with the PNG kept for a human to look at. */
function capture() {
  const raw = adb(["exec-out", "screencap"], { binary: true });
  if (raw.code !== 0) {
    throw new Error(`screencap failed (adb exited ${raw.code}): ${raw.stderr.trim()}`);
  }
  const frame = decodeFrame(raw.stdout);
  const png = adb(["exec-out", "screencap", "-p"], { binary: true });
  frame.png = png.code === 0 ? png.stdout : null;
  return frame;
}

/// What makes a frame something rather than nothing. Exported for the same
/// reason as [`decodeFrame`].
export function describe(frame) {
  const counts = new Map();
  for (const pixel of frame.pixels) counts.set(pixel, (counts.get(pixel) ?? 0) + 1);
  let modal = 0;
  for (const count of counts.values()) if (count > modal) modal = count;
  return {
    colours: counts.size,
    uniformity: modal / frame.pixels.length,
    blank: counts.size < MINIMUM_COLOURS || modal / frame.pixels.length > MAXIMUM_UNIFORMITY,
  };
}

/** The fraction of pixels that differ between two frames of the same size. */
export function changed(before, after) {
  if (before.width !== after.width || before.height !== after.height) return 1;
  let differing = 0;
  for (let at = 0; at < before.pixels.length; at += 1) {
    if (before.pixels[at] !== after.pixels[at]) differing += 1;
  }
  return differing / before.pixels.length;
}

const keep = (name, contents) => keepIn(options.out, name, contents);

async function main() {
  // ① The device, and that it has finished booting. Bounded rather than
  //    `adb wait-for-device`, for the reason `android-device.mjs` records.
  console.log(`device: ${await waitForBoot(options.settle)}`);

  // ② The control. Before anything of ours is on the device, so a failure here
  //    is the harness or the emulator and nothing else.
  const before = capture();
  const control = describe(before);
  await keep("before.png", before.png);
  console.log(`control frame: ${before.width}x${before.height}, ${control.colours} colours, `
    + `most common ${(control.uniformity * 100).toFixed(1)}%`);
  if (control.blank) {
    throw new Error("the frame captured before installing anything is blank, so this run "
      + "cannot tell a working application from a broken screenshot. The emulator or "
      + "`screencap` is the thing to look at, not Blitsen.");
  }

  // ③ Install, and confirm the application id is the one that arrived. A wrong
  //    `--package` would otherwise sail through `am start` and leave the
  //    launcher on screen, which the difference check below would catch — but
  //    it would report it as "Blitsen painted nothing", which is not true.
  adb(["logcat", "-c"]);
  const install = adb(["install", "-r", "-g", options.apk]);
  if (install.code !== 0 || !/Success/.test(install.stdout)) {
    throw new Error(`installing ${options.apk} failed\n  `
      + `${install.stdout.trim()}\n  ${install.stderr.trim()}`);
  }
  const path = adb(["shell", "pm", "path", options.package]).stdout.trim();
  if (!path.startsWith("package:")) {
    throw new Error(`the APK installed, but nothing is registered as ${options.package}. `
      + "Pass the application id the build reported to --package.");
  }
  console.log(`installed: ${path}`);

  // ④ Launch, then wait for a frame that is not the launcher. Polling rather
  //    than a fixed sleep so a fast device is not paid for, and so a device
  //    that dies is noticed at the moment it does.
  const started = adb(["shell", "am", "start", "-W", "-n",
    `${options.package}/${options.activity}`]);
  if (started.code !== 0 || /Error/.test(started.stdout)) {
    throw new Error(`am start ${options.package}/${options.activity} failed\n  `
      + started.stdout.trim());
  }
  console.log(started.stdout.trim().split("\n").filter(Boolean).map(line => `  ${line}`).join("\n"));

  const deadline = Date.now() + options.settle;
  let after = null;
  let reading = null;
  let motion = 0;
  while (Date.now() < deadline) {
    await sleep(2000);
    if (!deviceIsAlive()) {
      throw new Error("the emulator stopped answering adb while the application was "
        + "starting. This is #139's failure exactly: with a software Vulkan the emulator "
        + "died at wgpu initialisation rather than the application failing. Check the GPU "
        + "mode this ran under before looking at Blitsen.");
    }
    after = capture();
    reading = describe(after);
    motion = changed(before, after);
    if (!reading.blank && motion >= MINIMUM_CHANGE) break;
  }
  if (after === null) throw new Error("no frame was captured after launch");
  await keep("after.png", after.png);
  await keep("logcat.txt", adb(["logcat", "-d"]).stdout);

  console.log(`frame: ${after.width}x${after.height}, ${reading.colours} colours, `
    + `most common ${(reading.uniformity * 100).toFixed(1)}%, `
    + `${(motion * 100).toFixed(1)}% of pixels differ from the control`);

  // ⑤ The assertions, in the order that makes a failure readable: is the
  //    process still there, did it paint, and is what it painted its own.
  const pid = adb(["shell", "pidof", options.package]).stdout.trim();
  if (pid === "") {
    throw new Error(`${options.package} is not running any more. It started and then went `
      + "away, so read logcat.txt in the output directory for the tombstone.");
  }
  const crash = adb(["logcat", "-d", "-b", "crash"]).stdout.trim();
  if (crash !== "") throw new Error(`the crash log is not empty:\n${crash}`);
  if (reading.blank) {
    throw new Error(`${options.package} is running and the screen is blank: `
      + `${reading.colours} distinct colours, one of them covering `
      + `${(reading.uniformity * 100).toFixed(1)}% of the frame. The control frame taken `
      + "before the install was not blank, so the screenshot path works and this is a "
      + "frame the application did not paint.");
  }
  if (motion < MINIMUM_CHANGE) {
    throw new Error(`only ${(motion * 100).toFixed(1)}% of pixels differ from the frame `
      + "taken before the application was installed, so what is on screen is still the "
      + "launcher. The application is running and has not painted over it.");
  }
  console.log(`${options.package} (pid ${pid}) painted a frame.`);
  for (const artifact of artifacts) console.log(`  wrote ${artifact}`);
}

// Guarded so the three pure functions above can be imported and tested without
// this reaching for an emulator.
if (import.meta.main) {
  if (!options.apk || !options.package) {
    console.error("usage: run-android-smoke.mjs --apk <path> --package <application id> "
      + "[--activity <name>] [--serial <device>] [--out <dir>] [--settle <ms>]");
    process.exit(2);
  }
  try {
    await main();
  } catch (failure) {
    console.error(`android smoke: ${failure.message}`);
    await keep("logcat.txt", deviceIsAlive() ? adb(["logcat", "-d"]).stdout : null)
      .catch(() => {});
    for (const artifact of artifacts) console.error(`  wrote ${artifact}`);
    process.exit(1);
  }
}

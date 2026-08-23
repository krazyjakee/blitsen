// The `adb` half of the Android harnesses, in one place.
//
// `run-android-smoke.mjs` (#149) reads the framebuffer back; `run-android-notify.mjs`
// (#254) reads the notification shade back. What is between them is the same device:
// one shape of `adb` invocation, the same bounded wait for a boot, the same artifact
// directory a failing CI job has to be diagnosed from. This file is that middle,
// extracted when the second harness arrived rather than copied — two `spawnSync`
// wrappers diverge, and the one that diverges is the one whose failure nobody has
// read recently.
//
// Nothing either harness *measures* is here. Frame decoding stays in the smoke test
// and dumpsys parsing stays in the notification harness, because those are the
// assertions, and each is unit-tested next to the file that owns it.
import { spawnSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

/** One `--name value` off the command line. */
export function argument(name, fallback = null) {
  const at = process.argv.indexOf(`--${name}`);
  return at < 0 ? fallback : process.argv[at + 1];
}

/// Which device, when more than one is attached. `ANDROID_SERIAL` is adb's own
/// spelling of the same choice, so a caller that has already set it for adb does
/// not have to repeat itself on the command line.
const serial = argument("serial", process.env.ANDROID_SERIAL ?? null);

/** One `adb` call. Binary-safe: `exec-out` is used for anything that is bytes. */
export function adb(args, { binary = false } = {}) {
  const prefixed = serial ? ["-s", serial, ...args] : args;
  const result = spawnSync("adb", prefixed, {
    encoding: binary ? "buffer" : "utf8",
    maxBuffer: 256 * 1024 * 1024,
  });
  if (result.error) throw new Error(`adb ${args[0]}: ${result.error.message}`);
  return {
    code: result.status ?? 1,
    stdout: result.stdout ?? (binary ? Buffer.alloc(0) : ""),
    stderr: binary ? String(result.stderr ?? "") : (result.stderr ?? ""),
  };
}

/** `adb` that must succeed, and that says what it was doing when it did not. */
export function adbOrFail(args, what) {
  const result = adb(args);
  if (result.code !== 0) {
    throw new Error(`${what} failed (adb exited ${result.code})\n`
      + `  ${result.stderr.trim() || result.stdout.trim()}`);
  }
  return result.stdout;
}

export const sleep = milliseconds => new Promise(settle => setTimeout(settle, milliseconds));

/// Whether the device is still there at all.
///
/// Called after every step that could take it away, because #139's failure mode is
/// the emulator dying rather than the application misbehaving, and the two produce
/// completely different next actions.
export function deviceIsAlive() {
  const state = adb(["get-state"]);
  return state.code === 0 && state.stdout.trim() === "device";
}

/// Waits for a device that has finished booting, and returns its fingerprint.
///
/// Polled with a bound rather than `adb wait-for-device`, which waits for ever: a
/// job that hangs until the runner's own timeout kills it reports nothing, and
/// "there is no device" is a different answer from "the device never booted".
export async function waitForBoot(settle) {
  const booted = Date.now() + settle;
  while (!deviceIsAlive()) {
    if (Date.now() > booted) {
      throw new Error(`no device answered adb within ${settle} ms`
        + `${serial ? ` for serial ${serial}` : ""}. `
        + "Start an emulator, or pass --serial for the one to use.");
    }
    await sleep(1000);
  }
  while (adb(["shell", "getprop", "sys.boot_completed"]).stdout.trim() !== "1") {
    if (Date.now() > booted) throw new Error("the device never reported sys.boot_completed");
    await sleep(1000);
  }
  return adb(["shell", "getprop", "ro.build.fingerprint"]).stdout.trim();
}

/** Every artifact written so far, so a failing run can list what it left behind. */
export const artifacts = [];

/** Writes one artifact, and remembers it. A null body is nothing to write. */
export async function keep(directory, name, contents) {
  if (contents === null || contents === undefined) return;
  await mkdir(directory, { recursive: true });
  const path = join(directory, name);
  await writeFile(path, contents);
  artifacts.push(path);
}

/// The accessibility tree of whatever is on screen, as `uiautomator` XML.
///
/// Returns `null` rather than throwing, and callers poll: `uiautomator dump` fails
/// with "could not get idle state" whenever the window is still animating, which
/// during a permission dialog's entrance is most of the time. A dump that has to be
/// retried is normal; a dump that never succeeds is what the caller reports.
///
/// Written to the device and read back rather than dumped to `/dev/tty`, because
/// the tty path interleaves the tool's own progress line with the XML on some
/// builds and there is nothing to gain from parsing around it.
export function uiHierarchy() {
  const path = "/sdcard/blitsen-ui.xml";
  if (adb(["shell", "uiautomator", "dump", path]).code !== 0) return null;
  const xml = adb(["exec-out", "cat", path], { binary: true });
  return xml.code === 0 && xml.stdout.length > 0 ? xml.stdout.toString("utf8") : null;
}

const ENTITIES = { "&amp;": "&", "&lt;": "<", "&gt;": ">", "&quot;": "\"", "&apos;": "'" };

/// The nodes of a `uiautomator` dump, each with the point that taps it.
///
/// Exported for the reason the smoke test's frame decoder is: a parser that
/// silently finds nothing turns "the permission dialog never appeared" and "the
/// dialog appeared and this could not read it" into the same result, and only one
/// of those is a bug in Blitsen. `android-notify.test.mjs` holds it to real dump
/// text.
export function uiNodes(xml) {
  const nodes = [];
  for (const [, attributes] of xml.matchAll(/<node\s([^>]*?)\/?>/g)) {
    const read = name => {
      const found = new RegExp(`${name}="([^"]*)"`).exec(attributes);
      return found === null ? "" : found[1].replace(/&(amp|lt|gt|quot|apos);/g, e => ENTITIES[e]);
    };
    const bounds = /\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]/.exec(read("bounds"));
    if (bounds === null) continue;
    const [left, top, right, bottom] = bounds.slice(1).map(Number);
    nodes.push({
      id: read("resource-id"),
      text: read("text"),
      description: read("content-desc"),
      x: Math.round((left + right) / 2),
      y: Math.round((top + bottom) / 2),
      width: right - left,
      height: bottom - top,
    });
  }
  return nodes;
}

/** Taps the centre of a node from [`uiNodes`]. */
export function tap(node) {
  adbOrFail(["shell", "input", "tap", String(node.x), String(node.y)],
    `tapping ${JSON.stringify(node.text || node.id)}`);
}

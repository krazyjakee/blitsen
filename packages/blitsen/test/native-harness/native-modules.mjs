import { strict as assert } from "node:assert";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { arch, cpus, homedir, hostname, tmpdir, totalmem } from "node:os";
import { basename, isAbsolute, join } from "node:path";
import { loadApiManifest } from "../../src/api-manifest.mjs";
import app from "../../src/native/app.mjs";
import clipboard from "../../src/native/clipboard.mjs";
import dialog from "../../src/native/dialog.mjs";
import os from "../../src/native/os.mjs";
import windowModule from "../../src/native/window.mjs";

import { addonPath, native } from "./addon.mjs";

// The `native:` modules. Everything below reaches them the way an application
// does — through the `blitsen/app` and `blitsen/clipboard` proxies — so what is
// asserted is the installed namespace, not a description of it.
const nativeManifest = await loadApiManifest();
const namespaces = { app, clipboard, dialog, os, window: windowModule };
// The members whose presence is a platform fact rather than a version fact: the
// single-instance lock is a Unix socket, and a dialog is the XDG portal.
const absentOn = new Map([["app.requestSingleInstanceLock", ["win32"]]]);
for (const entry of nativeManifest.native.filter(entry => entry.module === "dialog")) {
  absentOn.set(entry.api, ["win32", "darwin"]);
}
for (const entry of nativeManifest.native) {
  const namespace = namespaces[entry.module];
  assert(namespace, `the manifest names native:${entry.module}, which the harness does not import`);
  const installed = entry.status === "implemented"
    && !(absentOn.get(entry.api) ?? []).includes(process.platform);
  if (installed) {
    assert.equal(typeof namespace[entry.member], "function",
      `native:${entry.api} is implemented and must be installed`);
    assert.equal(entry.member in namespace, true, `native:${entry.api} must be enumerable`);
  } else {
    // Absent, not stubbed: the property does not exist, so `if (app.onSuspend)`
    // selects a fallback instead of calling something that throws.
    assert.equal(namespace[entry.member], undefined,
      `native:${entry.api} is absent and must not be installed`);
    assert.equal(entry.member in namespace, false,
      `native:${entry.api} must not answer an "in" check`);
  }
}
for (const [name, namespace] of Object.entries(namespaces)) {
  assert.deepEqual(Object.keys(namespace).sort(),
    nativeManifest.native
      .filter(entry => entry.module === name && entry.status === "implemented"
        && !(absentOn.get(entry.api) ?? []).includes(process.platform))
      .map(entry => entry.member).sort(),
    `the native:${name} namespace enumerates exactly what the runtime installed`);
}
assert.throws(() => { app.dataDir = () => "/tmp"; }, /read-only/);

// Application directories. The application names itself, because the runtime
// cannot: the executable here is Bun.
const applicationName = "Blitsen Harness";
const directories = [app.dataDir(applicationName), app.cacheDir(applicationName),
  app.configDir(applicationName)];
for (const directory of directories) {
  assert.equal(isAbsolute(directory), true, `${directory} must be absolute`);
  assert.equal(basename(directory), applicationName);
}
assert.notEqual(directories[0], directories[1], "cache is not where data lives");
if (process.platform === "linux") {
  const home = (variable, fallback) => process.env[variable] || join(homedir(), fallback);
  assert.deepEqual(directories, [
    join(home("XDG_DATA_HOME", ".local/share"), applicationName),
    join(home("XDG_CACHE_HOME", ".cache"), applicationName),
    join(home("XDG_CONFIG_HOME", ".config"), applicationName),
  ], "the XDG base directories, or their defaults");
}
for (const rejected of ["", ".", "..", "escape/../..", "escape\\out"]) {
  assert.throws(() => app.dataDir(rejected), /not a valid application name/,
    `${JSON.stringify(rejected)} must not reach out of the directory the platform chose`);
}
// The directory is named, not created: making it is `node:fs`.
assert.equal(existsSync(directories[0]), false);

// The clipboard. A read is a round-trip through the real X11/Wayland selection
// or the system pasteboard; there is no in-process shortcut behind it.
assert.throws(() => clipboard.writeImage({ width: 2, height: 2, data: new Uint8Array(8) }),
  /RGBA bytes/, "an image must carry its own pixels");
export const displayed = process.platform !== "linux"
  || Boolean(process.env.DISPLAY || process.env.WAYLAND_DISPLAY);
if (displayed) {
  const text = `blitsen harness ${process.pid}`;
  clipboard.writeText(text);
  assert.equal(clipboard.readText(), text);
  clipboard.writeHtml("<b>bold</b>", "bold");
  // The markup survives; the document around it is the pasteboard's own. macOS
  // hands back a full `<html>` wrapping the fragment that was written, and a
  // paste target reads the same rendered result either way — so this asserts
  // the fragment arrived, not that the host declined to normalise it.
  assert.match(clipboard.readHtml(), /<b>bold<\/b>/);
  assert.equal(clipboard.readText(), "bold",
    "HTML carries the plain text a paste that cannot read it receives");
  const pixels = new Uint8Array([255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]);
  clipboard.writeImage({ width: 2, height: 2, data: pixels });
  const image = clipboard.readImage();
  assert.equal(image.width, 2);
  assert.equal(image.height, 2);
  assert.deepEqual([...image.data], [...pixels], "RGBA survives the clipboard's own encoding");
  assert.equal(clipboard.readText(), null, "an image is not text, and says so rather than throwing");
  clipboard.clear();
  assert.equal(clipboard.readText(), null);
  assert.equal(clipboard.readImage(), null);
} else {
  console.log("clipboard round-trips skipped: no DISPLAY or WAYLAND_DISPLAY on this host");
}

// What machine this is. None of it needs a window, so unlike the window and
// dialog sections below these are real end-to-end readings rather than refusals.
//
// Bun is hosting this harness, so `node:os` is here to check against — and it
// is a genuinely independent source, reading `sysinfo(2)` and `sysconf` where
// the bridge reads `/proc` through the `sysinfo` crate. Two libraries agreeing
// on the core count and the hostname is what rules out the failure a
// self-consistent reading cannot: plausible numbers about the wrong machine.
const processor = os.cpu();
assert.equal(processor.logicalCores, cpus().length, "the bridge counts the cores node:os counts");
assert.equal(processor.logicalCores, processor.cores.length);
assert.equal(processor.brand, cpus()[0].model.trim(), "and names the same processor");
// `x86_64` against node's `x64`: the same architecture in two vocabularies, so
// the check is a translation rather than an equality.
assert.equal(processor.architecture, { x64: "x86_64", arm64: "aarch64" }[arch()] ?? arch());
assert(processor.physicalCores === null || processor.physicalCores <= processor.logicalCores,
  `physical cores ${processor.physicalCores} cannot exceed logical ${processor.logicalCores}`);
for (const core of processor.cores) {
  assert(core.usage >= 0 && core.usage <= 100, `core usage ${core.usage} is out of range`);
  assert.equal(typeof core.frequency, "number");
}
// Usage is a delta against the previous call, which leaves the first call with
// nothing to measure from: it reports a baseline against the counters' own
// origin — on Linux the average since boot — rather than 0. So both calls are
// checked for range only. Neither is a number this can predict on a machine it
// does not control, and asserting one would be asserting how busy the host is.
assert(processor.usage >= 0 && processor.usage <= 100, `package usage ${processor.usage}`);
const resampled = os.cpu();
assert(resampled.usage >= 0 && resampled.usage <= 100, `package usage ${resampled.usage}`);

const memory = os.memory();
assert(memory.total > 0);
assert(memory.used <= memory.total, `${memory.used} used of ${memory.total}`);
assert(memory.available <= memory.total);
assert(memory.swapUsed <= memory.swapTotal);
// Node reads the same number through a different syscall; a 2% band absorbs
// that without letting a wrong machine through.
assert(Math.abs(memory.total - totalmem()) / totalmem() < 0.02,
  `${memory.total} bytes installed, and node:os says ${totalmem()}`);

const volumes = os.storage();
assert(volumes.length > 0, "something is mounted");
assert(volumes.some(volume => volume.total > 0), "and at least one mount has capacity");
for (const volume of volumes) {
  assert(volume.mountPoint.length > 0, "every volume says where it is mounted");
  assert(volume.available <= volume.total, `${volume.mountPoint}: ${volume.available}/${volume.total}`);
  assert(["ssd", "hdd", "unknown"].includes(volume.kind), `${volume.mountPoint}: ${volume.kind}`);
}

const machine = os.host();
assert.equal(machine.hostName, hostname(), "the bridge names the host node:os names");
assert(machine.bootTime > 0);
assert(machine.uptime > 0);
assert(machine.distributionId.length > 0);

// The window, and the dialogs that are modal to it.
//
// This harness loads the addon into Bun rather than into a `blitsen <directory>`
// run, so there is no window here and never will be. That is the honest half to
// assert: everything each call decides before it needs one — the vocabulary it
// accepts and the shape of its options — plus the fact that a call without a
// window says which it is instead of quietly doing nothing. Driving a real
// window, or dismissing a real dialog, needs a person; the M4 notes say what to
// run and what to look for.
for (const [call, refusal] of [
  [() => windowModule.setCursor("wiggly"), /not a CSS cursor keyword/],
  [() => windowModule.setCursorGrab("everything"), /not a cursor grab mode/],
  [() => windowModule.setSize(0, 100), /at least 1x1 CSS pixels/],
  [() => windowModule.setSize(800, Infinity), /at least 1x1 CSS pixels/],
  [() => windowModule.setSize("wide", 600), /invalid window width/],
]) assert.throws(call, refusal, "a mistyped argument is refused before the window is looked for");

for (const call of [
  () => windowModule.setSize(800, 600),
  () => windowModule.setFullscreen(true),
  () => windowModule.isFullscreen(),
  () => windowModule.setDecorations(false),
  () => windowModule.isDecorated(),
  () => windowModule.setAlwaysOnTop(true),
  () => windowModule.setCursor("pointer"),
  () => windowModule.setCursorVisible(false),
  () => windowModule.setCursorGrab("none"),
  () => windowModule.monitors(),
]) assert.throws(call, /no application window yet/,
  "a window operation with no window reports that, rather than being a no-op");

if (dialog.openFile) {
  for (const [call, refusal] of [
    [() => dialog.openFile(null), /options must be an object/],
    [() => dialog.openFile({ filters: "text" }), /filters must be an array/],
    [() => dialog.openFile({ filters: [{ name: "text" }] }), /name, extensions/],
    [() => dialog.message({ level: "shouting" }), /not a message level/],
    [() => dialog.message({ buttons: "maybe" }), /not a button set/],
  ]) assert.throws(call, refusal, "dialog options are checked where the call was made");
  // A dialog here is always modal to the application window, so without one
  // nothing opens — and nothing is left outstanding for a frame turn to deliver.
  const outstanding = globalThis.__blitsenAnimationFramesPending();
  for (const call of [
    () => dialog.openFile({ title: "Open", filters: [{ name: "Text", extensions: ["txt"] }] }),
    () => dialog.openFiles(),
    () => dialog.saveFile({ fileName: "untitled.txt" }),
    () => dialog.openFolder({ directory: tmpdir() }),
    () => dialog.openFolders(),
    () => dialog.message({ title: "Quit", message: "Really?", buttons: "yesNo" }),
  ]) assert.throws(call, /no application window yet/);
  assert.equal(globalThis.__blitsenAnimationFramesPending(), outstanding,
    "a dialog that never opened leaves nothing for a frame turn to settle");
}

// The single-instance lock, over the real socket: the second request finds the
// lock held, hands this invocation over, and the first instance is handed it
// back on a frame turn.
if (process.platform !== "win32") {
  const received = [];
  // A stable name, so a run after one that crashed also exercises taking over a
  // socket whose owner is gone.
  const lockName = "blitsen-native-harness";
  assert.equal(app.requestSingleInstanceLock(lockName, invocation => received.push(invocation)),
    true, "the first instance owns the lock");
  assert.throws(() => app.requestSingleInstanceLock(lockName, "not a function"), TypeError);
  assert.equal(app.requestSingleInstanceLock(lockName), false,
    "a second request finds the lock held and hands its invocation over");
  // The hand-off crosses a socket and a listener thread, so the wait is for the
  // host to report work; the delivery itself is one turn, not a poll.
  let waiting = false;
  for (let turn = 0; turn < 200 && !waiting; turn++) {
    waiting = globalThis.__blitsenAnimationFramesPending();
    if (!waiting) await Bun.sleep(5);
  }
  assert.equal(waiting, true, "an undelivered invocation keeps the host turning");
  assert.equal(received.length, 0, "nothing is delivered between frame turns");
  globalThis.__blitsenAnimationFrameTick(0);
  assert.equal(received.length, 1, "the invocation arrived on the next frame turn");
  assert.deepEqual(received[0].argv, process.argv.map(String),
    "the second instance's command line, as the OS gave it");
  assert.equal(received[0].cwd, process.cwd());
  assert.equal(globalThis.__blitsenAnimationFramesPending(), false,
    "a delivered invocation stops asking for frames");
}

// `relaunch`. The successor is this process's own command line run again, so it
// is tested with a script that counts its own generations and stops at two.
const relaunchDirectory = mkdtempSync(join(tmpdir(), "blitsen-relaunch-"));
try {
  const marker = join(relaunchDirectory, "generations");
  const script = join(relaunchDirectory, "relaunch.mjs");
  writeFileSync(marker, "");
  writeFileSync(script, `
    import { appendFileSync, readFileSync } from "node:fs";
    import { createRequire } from "node:module";
    const native = createRequire(import.meta.url)(process.env.BLITSEN_RELAUNCH_ADDON);
    native.runBridgeHarness("<div></div>", "", 32, 32);
    const { default: app } = await import(process.env.BLITSEN_RELAUNCH_MODULE);
    const marker = process.env.BLITSEN_RELAUNCH_MARKER;
    appendFileSync(marker, process.argv.join(" ") + "\\n");
    const generations = readFileSync(marker, "utf8").split("\\n").filter(Boolean);
    if (generations.length < 2) app.relaunch();
  `);
  const relaunched = Bun.spawnSync({
    cmd: [process.execPath, script],
    env: {
      ...process.env,
      BLITSEN_RELAUNCH_ADDON: addonPath,
      BLITSEN_RELAUNCH_MARKER: marker,
      BLITSEN_RELAUNCH_MODULE: new URL("../../src/native/app.mjs", import.meta.url).href,
    },
    stdout: "inherit",
    stderr: "inherit",
  });
  assert.equal(relaunched.exitCode, 0, "the relaunching process exits cleanly");
  let generations = [];
  for (let wait = 0; wait < 200 && generations.length < 2; wait++) {
    await Bun.sleep(25);
    generations = readFileSync(marker, "utf8").split("\n").filter(Boolean);
  }
  assert.equal(generations.length, 2, "relaunch starts a successor that outlives this process");
  assert.equal(generations[0], generations[1],
    "the successor runs the same command line, argument for argument");
} finally {
  rmSync(relaunchDirectory, { recursive: true, force: true });
}

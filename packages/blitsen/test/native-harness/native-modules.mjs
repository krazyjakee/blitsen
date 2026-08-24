import { strict as assert } from "node:assert";
import {
  existsSync, mkdtempSync, readFileSync, readdirSync, readlinkSync, rmSync, writeFileSync,
} from "node:fs";
import { arch, cpus, homedir, hostname, tmpdir, totalmem } from "node:os";
import { basename, isAbsolute, join } from "node:path";
import { loadApiManifest } from "../../src/api-manifest.mjs";
import app from "../../src/native/app.mjs";
import clipboard from "../../src/native/clipboard.mjs";
import dialog from "../../src/native/dialog.mjs";
import hid from "../../src/native/hid.mjs";
import input from "../../src/native/input.mjs";
import menu from "../../src/native/menu.mjs";
import notify from "../../src/native/notify.mjs";
import os from "../../src/native/os.mjs";
import tray from "../../src/native/tray.mjs";
import windowModule from "../../src/native/window.mjs";

import { addonPath, native } from "./addon.mjs";

// The `native:` modules. Everything below reaches them the way an application
// does — through the `blitsen/app` and `blitsen/clipboard` proxies — so what is
// asserted is the installed namespace, not a description of it.
const nativeManifest = await loadApiManifest();
const namespaces = { app, clipboard, dialog, hid, input, menu, notify, os, tray, window: windowModule };
// The members whose presence is a platform fact rather than a version fact: a
// dialog is the XDG portal, and an application menu is the macOS main menu or
// the Windows menu bar. The single-instance transport differs by platform but
// the member is present on every desktop target.
const absentOn = new Map();
for (const entry of nativeManifest.native.filter(entry => entry.module === "dialog")) {
  absentOn.set(entry.api, ["win32", "darwin"]);
}
for (const entry of nativeManifest.native.filter(entry => entry.module === "menu")) {
  absentOn.set(entry.api, ["linux"]);
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

// Every asynchronous native capability uses one command/completion protocol.
// Exercise two independent channels with the same command ID so a completion
// can neither cross capability boundaries nor be captured by a stale message.
const commandChannels = JSON.parse(native.runBridgeHarness(
  `<div id="channels"></div>`,
  `{ const makeChannel = globalThis.__blitsenTestCommandChannel;
     if (typeof makeChannel !== "function") throw new Error("missing test command-channel seam");
     const trayWire = [];
     const hidWire = [];
     const seen = [];
     const trayChannel = makeChannel({
       pending: () => trayWire.length > 0,
       take: () => trayWire.splice(0),
       result: message => message.value,
       onMessage: message => seen.push(message.type),
     });
     const hidChannel = makeChannel({
       pending: () => hidWire.length > 0,
       take: () => hidWire.splice(0),
       error: message => {
         seen.push(message.errorName + ":" + message.error);
         return new DOMException(message.error, message.errorName);
       },
     });
     void trayChannel.run("1", { onComplete: value => seen.push("tray:" + value) });
     void hidChannel.run("1").catch(() => {});
     trayWire.push(
       { type: "completion", commandId: "stale", value: "wrong", error: null },
       { type: "click" },
       { type: "completion", commandId: 1, value: "ready", error: null },
     );
     trayChannel.settle();
     if (!hidChannel.workPending()) throw new Error("one channel consumed another's command ID");
     hidWire.push({ type: "completion", commandId: 1, value: null,
       error: "access denied", errorName: "NotAllowedError" });
     hidChannel.settle();

     const dialogWire = [{ id: 3, value: [] }];
     const dialogChannel = makeChannel({
       take: () => dialogWire.splice(0), completion: () => true,
       commandId: answer => answer.id, result: answer => answer.value,
       rejected: () => false, pollPendingCommands: true,
     });
     let cancelled = "unsettled";
     void dialogChannel.run(3, { transform: paths => (cancelled = paths[0] ?? null) });
     dialogChannel.settle();
     const expected = ["click", "tray:ready", "NotAllowedError:access denied"];
     if (JSON.stringify(seen) !== JSON.stringify(expected) || cancelled !== null
       || trayChannel.workPending() || hidChannel.workPending() || dialogChannel.workPending())
       throw new Error("command channels lost FIFO, errors, cancellation, or isolation: "
         + JSON.stringify({ seen, cancelled }));
     document.getElementById("channels").setAttribute("data-result", "passed"); }`,
  32,
  32,
));
assert.equal(commandChannels.nodes.find(node => node.attributes.id === "channels")
  .attributes["data-result"], "passed", "shared native command channels preserve their contracts");

const inputState = input.snapshot();
assert.equal(typeof inputState.sequence, "number");
assert.equal(typeof inputState.focused, "boolean");
assert(Array.isArray(inputState.keys));
assert(Array.isArray(inputState.pointer.buttons));

// Tray configuration is applied by a window session, which this module-only
// harness deliberately does not open. Invalid trees still travel through the
// public normalizer and Rust parser synchronously, so they exercise the full
// boundary without leaving a pending native request behind.
assert.throws(() => tray.configure({
  icon: new Uint8Array(),
  menu: [
    { type: "radio", id: "light", label: "Light", group: "theme" },
    { type: "radio", id: "dark", label: "Dark", group: "theme" },
  ],
}), /exactly one checked item/);
assert.throws(() => tray.configure({
  icon: new Uint8Array(),
  menu: [{ id: "open", label: "Open", accelerator: "KeyO+Control" }],
}), /modifiers must precede one key/);
assert.throws(() => tray.configure({
  icon: new Uint8Array(),
  menu: [
    { action: "separator" },
    { id: "open", label: "Open", accelerator: "KeyO+Control" },
  ],
}), /modifiers must precede one key/,
"the legacy tray separator is normalized before the following entry is validated");
assert.throws(() => tray.configure({
  icon: new Uint8Array(),
  menu: [{ id: "open", label: "Open", icon: new Uint8Array([1, 2, 3]) }],
}), /not a valid PNG/);

// The application menu validates the same way, and on the platforms that have
// one it does so without a tray icon ever being configured — which is the whole
// point of its being a separate module.
if (menu.configure) {
  assert.throws(() => menu.configure({ menu: [{ id: "open", label: "Open" }] }),
    /every top-level application menu entry must be a submenu/);
  assert.throws(() => menu.configure({
    menu: [{ type: "submenu", label: "Edit", menu: [{ type: "role", role: "explode" }] }],
  }), /unknown application menu role/);
  assert.throws(() => menu.configure({
    menu: [{ type: "submenu", label: "File", menu: [{ action: "quit" }] }],
  }), /application menu action id/);
  assert.throws(() => menu.configure({
    menu: [{ type: "submenu", label: "File", menu: [{ action: "separator" }] }],
  }), /application menu action id/,
  "legacy action separators remain a tray-only compatibility spelling");
  assert.throws(() => menu.configure({
    menu: [
      { type: "submenu", role: "edit", label: "Edit", menu: [] },
      { type: "submenu", role: "edit", label: "Also Edit", menu: [] },
    ],
  }), /declares the edit role twice/);
}

// Notification validation also crosses the public JavaScript normalizer and
// Rust parser before a window session is needed to submit the command.
assert.throws(() => notify.show({
  title: "Build complete",
  actions: [{ id: "open", title: "Open" }, { id: "open", title: "Again" }],
}), /reserved or duplicated/);
assert.throws(() => notify.show({ title: "Build complete", timeout: -1 }), /non-negative/);
assert.throws(() => notify.update("n1", { title: "" }), /must not be empty/);

// Raw HID enumeration and every transfer settle on a frame turn, which a
// module-only harness has no window session to turn — so a `devices()` here
// would park a request rather than answer one. What does cross the whole
// boundary synchronously is the listener contract and the watch flag it sets in
// the host, and the real device tree is enumerated by the Rust hardware smoke.
assert.throws(() => hid.onDeviceChange("not a function"), /must be a function/);
const unwatch = hid.onDeviceChange(() => {});
assert.equal(typeof unwatch, "function");
unwatch();

// The standard facade is installed only where the same backend can address
// close, which is Linux and — since #251 tagged the toast — Windows. An
// unbundled macOS harness may still lack the application identity Apple
// requires, so it is not asserted either way there.
if (process.platform === "linux" || process.platform === "win32") {
  assert.equal("Notification" in globalThis, true);
  assert.equal(Notification.maxActions, 8);
  assert(Notification.prototype instanceof EventTarget);
  assert.throws(() => new Notification("Build", { tag: "replace-me" }), /NotSupportedError/);
  if (process.platform === "linux") {
    // Linux has no per-application authorization state to report.
    assert.equal(Notification.permission, "granted");
  } else {
    // Windows reads the notifier, which is enabled or switched off and never
    // undetermined; which of the two is a property of the machine. A machine
    // that registered no AppUserModelID for this process holds no notifier at
    // all, and #251 refuses that with the prerequisite rather than inventing a
    // verdict — so the contract is one of those two answers, not one of them
    // and a crash. A bare `bun` process on a stripped image (a CI runner,
    // Server Core) is the second case, which is why it is asserted rather than
    // stepped around.
    let permission;
    try {
      permission = Notification.permission;
    } catch (error) {
      assert.match(String(error.message), /AppUserModelID/,
        `Windows notification permission failed for a reason other than a missing identity: ${error.message}`);
      permission = null;
    }
    assert(permission === null || ["granted", "denied"].includes(permission),
      `Windows notification permission was ${permission}`);
  }
}

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
// Neither library invents a name, and they read different sources — so on two
// of the six targets one of them has nothing to say. node reads
// /proc/cpuinfo, which carries no `model name` on arm64 Linux, and answers
// "unknown" where the bridge reads the implementer and part registers and
// answers "Neoverse-N2"; the bridge reads the registry through sysinfo, which
// is empty on arm64 Windows, where node answers "Cobalt 100" (#137).
//
// Silence from the bridge is `null` rather than `""`, which is the shape the
// rest of this module uses for a fact the platform will not report, and the
// assertion here holds it to that: an empty string would mean the mapping
// stopped happening.
//
// What holds on all six is that they never name *different* processors, which
// is the failure this pair exists to rule out: plausible strings about the
// wrong machine. Silence from either side is a platform fact, not a mismatch.
for (const [field, value] of [["brand", processor.brand], ["vendor", processor.vendor]]) {
  assert(value === null || (typeof value === "string" && value !== ""),
    `cpu().${field} is ${JSON.stringify(value)}: an unreported name is null, never empty`);
}
const nodeBrand = cpus()[0].model.trim();
const bridgeBrand = processor.brand ?? "";
const unnamed = bridgeBrand === "" || nodeBrand === "" || nodeBrand === "unknown";
assert(unnamed || bridgeBrand === nodeBrand,
  `the bridge says ${JSON.stringify(bridgeBrand)} and node:os says ${JSON.stringify(nodeBrand)}`);
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

// Power. Whether this machine has a battery is not something the test can
// arrange — the same file runs on a laptop and on a build server — so the claim
// is that asking succeeded and that everything it answered describes a real
// battery. The empty list a desktop gives is the point rather than a skip: it is
// the reading that had to be told apart from a failure to ask, which is why a
// machine that cannot be asked throws here instead (#98).
const batteries = os.batteries();
assert(Array.isArray(batteries), "the batteries are a list, empty on a machine with none");
for (const battery of batteries) {
  assert(battery.level >= 0 && battery.level <= 1, `charge ${battery.level} is not a share`);
  assert(["charging", "discharging", "empty", "full", "unknown"].includes(battery.state),
    `${battery.state} is not a battery state`);
  // Health may exceed 1 where the design figure is conservative, so the check is
  // that a capacity was read at all rather than that it fits in a range.
  assert(battery.health > 0, `${battery.health} of its design capacity`);
  assert(battery.timeToFull === null || battery.timeToEmpty === null,
    "a battery is either filling or emptying, and the estimate is for the one it is doing");
  for (const name of [battery.vendor, battery.model])
    assert(name === null || (typeof name === "string" && name !== ""),
      `${JSON.stringify(name)}: an unreported name is null, never empty`);
}
// Linux publishes the same devices this reads under sysfs, so the count is
// checkable against the kernel rather than only against itself. `scope` is what
// keeps a wireless mouse's cell out of the list: a peripheral's battery is not
// one this machine runs on, and reading the directory the same way the bridge
// does is what proves that filter is applied rather than assumed.
if (process.platform === "linux") {
  const root = "/sys/class/power_supply";
  const supply = (device, file) => {
    try { return readFileSync(join(root, device, file), "utf8").trim(); }
    catch { return null; }
  };
  const system = (existsSync(root) ? readdirSync(root) : []).filter(device =>
    supply(device, "type") === "Battery" && (supply(device, "scope") ?? "System") === "System");
  assert.equal(batteries.length, system.length,
    `the bridge reports ${batteries.length} batteries and sysfs has ${system.length}`);
}

// The locale. `Intl` in this process is the addon's own rather than the host
// JavaScript engine's — the bridge installs it over whatever was there — so
// these two are checked against each other because they are *documented to
// agree* (COMPATIBILITY.md), not because they are independent. Both values are
// specified as ones an application hands straight to a formatter, and the first
// two assertions are that claim rather than a restatement of it.
const locale = os.locale();
assert.deepEqual(Intl.getCanonicalLocales(locale.language), [locale.language],
  `${locale.language} is not a canonical BCP-47 tag`);
assert.doesNotThrow(() => new Intl.DateTimeFormat(locale.language, { timeZone: locale.timeZone }),
  "the reported locale and zone are values the formatters accept");
// Compared by what the zone does rather than by its name: the same zone has
// more than one IANA spelling — `Europe/Kyiv` and `Europe/Kiev` — and what an
// application sees is the formatted result.
const resolved = new Intl.DateTimeFormat().resolvedOptions();
const inZone = zone => new Intl.DateTimeFormat("en-US",
  { timeZone: zone, dateStyle: "short", timeStyle: "long" }).format(new Date(0));
assert.equal(inZone(locale.timeZone), inZone(resolved.timeZone),
  `the bridge is in ${locale.timeZone} and Intl resolved ${resolved.timeZone}`);
// The independent source, where this machine is configured in a way the test
// can read without asking the same library twice: `TZ` when it names a zone,
// and otherwise the `/etc/localtime` symlink. Neither is guaranteed to be
// there — a copied `/etc/localtime`, a `TZ` holding a POSIX rule — and a
// machine that has neither is not a failure, so the check is skipped rather
// than guessed at.
const zoneName = /^[A-Za-z]+\/[\w+\-/]+$/;
const configuredZone = () => {
  if (zoneName.test(process.env.TZ ?? "")) return process.env.TZ;
  try {
    return readlinkSync("/etc/localtime").split("/zoneinfo/")[1] ?? null;
  } catch { return null; }
};
const configured = configuredZone();
if (configured) {
  assert.equal(inZone(locale.timeZone), inZone(configured),
    `the bridge is in ${locale.timeZone} and this machine is configured for ${configured}`);
}

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
  () => windowModule.setMinimized(true),
  () => windowModule.setMaximized(true),
  () => windowModule.isMaximized(),
  () => windowModule.startDrag(),
  () => windowModule.close(),
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

// The single-instance lock, over the real Unix socket or Windows named pipe:
// the second request finds the lock held, hands this invocation over, and the
// first instance is handed it back on a frame turn.
{
  const received = [];
  const lockName = `blitsen-native-harness-${process.pid}`;
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

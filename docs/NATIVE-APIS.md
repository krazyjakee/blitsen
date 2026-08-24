# Native APIs

Blitsen exposes operating-system capabilities as package subpaths. Use them when the web platform
has no suitable API, and keep ordinary DOM/web APIs for everything else.

## Import a native module

The recommended import form works with normal npm-aware bundlers:

```js
import clipboard from "blitsen/clipboard";
import windowApi from "blitsen/window";
```

Every member is optional because the running version or target may not implement it. Detect the
member before calling it:

```js
if (clipboard.writeText) {
  clipboard.writeText("Copied from Blitsen");
}
```

Outside Blitsen, accessing a native module member throws. Put browser-preview behavior behind an
environment boundary or a dynamic import rather than expecting the module to act as a browser
polyfill.

## Available modules

| Import | Available members in this release |
| --- | --- |
| `blitsen/app` | `dataDir`, `cacheDir`, `configDir`, `requestSingleInstanceLock`, `relaunch` |
| `blitsen/window` | `setSize`, `setFullscreen`, `isFullscreen`, `setDecorations`, `isDecorated`, `setMinimized`, `setMaximized`, `isMaximized`, `startDrag`, `close`, `setAlwaysOnTop`, `setCursor`, `setCursorVisible`, `setCursorGrab`, `monitors` |
| `blitsen/dialog` | `openFile`, `openFiles`, `saveFile`, `openFolder`, `openFolders`, `message` |
| `blitsen/clipboard` | `readText`, `readHtml`, `readImage`, `writeText`, `writeHtml`, `writeImage`, `clear` |
| `blitsen/tray` | `configure`, `remove`, `onClick`, `onAction` |
| `blitsen/menu` | `configure`, `remove`, `onAction` |
| `blitsen/notify` | `show`, `permission`, `requestPermission`, `update`, `close`, `onEvent` |
| `blitsen/input` | `snapshot` |
| `blitsen/hid` | `devices`, `open`, `onDeviceChange` |
| `blitsen/os` | `cpu`, `memory`, `storage`, `host`, `batteries`, `locale` |

The declaration files installed with `blitsen` document parameters and result types. The
[generated native module matrix](COMPATIBILITY.md#native-modules) is available when you need the
exact per-member runtime manifest.

## Window lifetime

`blitsen/window` controls the native window belonging to the calling document. Window and dialog
methods need that window, which exists from the `load` event onward:

```js
import windowApi from "blitsen/window";

addEventListener("load", () => {
  windowApi.setFullscreen?.(true);
});
```

Calling these methods from a document script before the window exists throws instead of silently
doing nothing.

There is deliberately no `window.create` in this release. The multi-window architecture is
[decided](TECH.md#multi-window-contexts-isolated-on-one-ui-thread), but its required per-window
host state is not implemented yet. When multiple native windows arrive, each will have an isolated
`Window`, `Document`, JavaScript heap and evaluated module graph while every window remains on the
same OS UI thread. A caller will not receive another context's global or DOM. Application data will
cross by structured-cloned `postMessage` calls on an explicitly transferred `MessagePort`, and a
separate opaque lifecycle capability will identify the native window.

The application session owns each future window. Closing the context that requested one will not
implicitly close it; closing the target will dispose only its context, workers and owned ports.
Creation/startup failure will reject the future creation operation without damaging an existing
window, while later uncaught errors remain attributed to their own window. Those are requirements
on an eventual API, not members available for feature detection today.

## Tray lifecycle

Package configuration still creates the tray before application JavaScript runs. The runtime API
operates on that same tray: `configure` creates or atomically replaces it, and `remove` destroys it.
Runtime icons are PNG file contents rather than paths, so their meaning does not change between a
development directory and a standalone export.

```js
import tray from "blitsen/tray";

const icon = new Uint8Array(await (await fetch("/tray.png")).arrayBuffer());
await tray.configure?.({
  icon,
  tooltip: "My App",
  menu: [
    { id: "open", label: "Open", accelerator: "CmdOrCtrl+KeyO" },
    { type: "checkbox", id: "launch", label: "Launch at login", checked: true },
    {
      type: "submenu",
      label: "Theme",
      menu: [
        { type: "radio", id: "light", label: "Light", group: "theme", checked: true },
        { type: "radio", id: "dark", label: "Dark", group: "theme" },
      ],
    },
    { type: "separator" },
    { action: "quit" },
  ],
});

tray.onAction?.(({ id, checked }) => {
  if (id === "open") console.log("Open selected");
  if (id === "launch") console.log("Launch at login:", checked);
});
```

The built-in `show`, `hide` and `quit` actions run in the native session even when JavaScript is
not currently painting. Application-defined IDs are delivered in FIFO order at a frame boundary.
IDs are unique across the full tree. Checkbox events carry their new state; each consecutive radio
group must start with exactly one checked item, and selecting one reports `checked: true` after the
native menu has cleared its siblings. Reconfigure with the new tree to persist application state.

Action and submenu icons are PNG bytes. Checkable-item icons and item visibility are deliberately
not exposed: the installed Windows/macOS menu backend cannot represent them consistently. Native
accelerators use modifier-first spellings such as `Control+Shift+KeyP`; `CmdOrCtrl` maps to Command
on macOS and Control elsewhere. Package configuration accepts the same menu tree and invariants,
using PNG paths relative to its `package.json` where the runtime API uses unambiguous byte arrays.
Configured custom and checkable actions queue until application listeners are installed.
Tray support is desktop-only.

## Application menu

`blitsen/menu` is the macOS main menu and the Windows window menu bar. It is a separate module from
`blitsen/tray` because it is a separate object with a separate owner: an application that shows no
status item at all still has one, and replacing one never disturbs the other.

An application menu is a bar, so every top-level entry is a submenu. Below that the tree is the
tray's — nested submenus, checkboxes, radio groups, separators and accelerators, with IDs unique
across the whole tree — plus role items, and minus icons and the tray's `show`/`hide`/`quit`
actions.

```js
import menu from "blitsen/menu";

await menu.configure?.({
  menu: [
    { type: "submenu", role: "application", label: "My App", menu: [
      { type: "role", role: "about" },
      { type: "separator" },
      { type: "role", role: "quit" },
    ] },
    { type: "submenu", label: "File", menu: [
      { id: "new", label: "New", accelerator: "CmdOrCtrl+KeyN" },
      { type: "checkbox", id: "autosave", label: "Autosave", checked: true },
    ] },
    { type: "submenu", role: "help", label: "Help", menu: [{ id: "docs", label: "Documentation" }] },
  ],
});

menu.onAction?.(({ id, checked }) => {
  if (id === "new") console.log("New document");
  if (id === "autosave") console.log("Autosave:", checked);
});
```

A `role` item is a command the platform performs itself — `copy`, `undo`, `minimize`, `quit` and the
rest — and never reaches application JavaScript. That is the point of it: an application that
implemented `paste` itself would implement it wrongly, because on macOS it is a menu command sent
down the responder chain rather than a key event. `services`, `showAll`, `hideOthers`, `fullscreen`
and `bringAllToFront` are macOS commands; on Windows the item is omitted rather than shown dead.
Application-defined items carry an `id` instead and are delivered in FIFO order at a frame boundary,
exactly as tray actions are.

On macOS, `role` on a *top-level submenu* claims one of the four positions AppKit reads by position
rather than by title. Blitsen supplies a standard `application`, `edit` and `window` submenu for
each role the application did not claim, because without them there is no About or Quit anywhere and
⌘C and ⌘V do nothing in a text field. The synthesized `application` submenu is always first, the
`window` submenu is always second to last, and a synthesized `edit` submenu goes immediately before
it; a submenu the application declares with a role is moved into that place instead, and a declared
`edit` submenu keeps the position the application chose. A `help` submenu is placed last and never
synthesized: its role is a position, there is no predefined command to put in one, and a submenu
with nothing in it is a greyed-out title. Windows installs the tree as written, with no synthesis.

`configure` replaces the whole menu in one step and `remove` takes it away. Both are atomic: the
replacement is built before the outgoing menu is detached, so a tree the platform refuses leaves the
running menu exactly as it was, and a click the platform had already queued against the outgoing
menu is dropped rather than delivered to the replacement.

Package configuration installs a menu before application JavaScript runs, under the `menu` key of
the `blitsen` config; the runtime API replaces that same menu rather than adding a second one. See
[CONFIGURATION.md](CONFIGURATION.md#application-menu).

There is no application menu on Linux or Android, and the module is feature-detectably absent
there — `menu.configure` is `undefined`. See
[PLATFORM-SUPPORT.md](PLATFORM-SUPPORT.md#application-menu) for the argument.

## Notifications

Notification work is asynchronous and settles on a frame turn. `show` returns a session-scoped ID;
that same ID appears in lifecycle events and addresses replacement and close where the platform
backend supports them:

```js
import notify from "blitsen/notify";

const unsubscribe = notify.onEvent?.(event => {
  if (event.type === "action" && event.action === "open") openArchive();
  if (event.type === "close") console.log(event.reason);
});

const id = await notify.show?.({
  title: "Export complete",
  body: "The archive is ready.",
  urgency: "normal",
  actions: [{ id: "open", title: "Open archive" }],
});

await notify.update?.(id, { body: "Copied to Downloads" });
await notify.close?.(id);
```

`permission()` reads without prompting; `requestPermission()` prompts on macOS and Android 13+
and otherwise reads the platform result. Linux returns `"granted"` because neither its freedesktop
service nor notification portal exposes a permission prompt—an unavailable service still makes
`show` reject. macOS
requires a signed `.app` bundle identity for authorization and uses its application icon;
passing `icon` is rejected. A Linux development run accepts an icon theme name or absolute path;
an identified export uses the notification portal, which accepts a theme name but requires a
sealed file descriptor for an image, so this API rejects its absolute paths rather than sending the
unsupported `file` variant. Windows accepts an image path. Android accepts an application drawable
resource name and otherwise uses a system fallback icon.

An exported macOS application has that identity. A development run does not: `blitsen run` is an
interpreter executing a script, so `permission`, `requestPermission` and `show` reject with a
message naming `blitsen run --dev-bundle`, which builds a signed development `.app` around the
interpreter and re-runs the same command inside it. That bundle's identifier is the development
host's own—`com.blitsen.dev.<name>` unless `--bundle-id` names another—and never an installed
application's, so a permission granted in development is not one granted to what you ship. See
[Packaging](PACKAGING.md#run-with-a-macos-development-identity).

IDs start again in each application session. Once a notification is clicked, acted on, dismissed,
expired or closed, `update` and `close` return `false`. Events are FIFO and are never delivered from
a platform callback thread. At most eight action buttons may be requested; the desktop is still
free to show fewer.

Every desktop platform implements update and close. Windows carries the session ID as the toast's
own tag, so an update replaces that toast in place and a close removes it from the screen and from
notification history alike. Windows permission reads the native notifier setting and is therefore
`"granted"` or `"denied"` and never `"default"`; there is no prompt to show, because the user,
the administrator and group policy are what decide it. Windows toasts are delivered under the
identity Windows already knows rather than under `appName`. An export built with `--bundle-id`
registers an identity of its own at startup and files its toasts under that instead; a development
run borrows Windows PowerShell's, which is what an unregistered process has always used. A machine
with no registered AppUserModelID at all keeps no notifier and therefore no setting, so both calls
reject there with a message naming that missing identity instead of reporting `"denied"`, which
would claim a decision nobody made.
On Linux, a development run retains the freedesktop backend and its running-process click, action,
dismissal and expiry events. An export with `--bundle-id` instead uses the notification portal so a
body click or named action can D-Bus-activate a stopped application. The portal supports replacement
by ID and removal, but exposes neither a dismissal/expiry callback nor a timeout field; for packaged
applications the desktop owns presentation lifetime, `appName` is the installed desktop identity,
and only explicit `close` produces a close event. Its `icon` is an installed icon-theme name; unlike
the live-process backend, an absolute image path is rejected because the portal accepts image files
only through a sealed file descriptor.
Android implements permission, an idempotent `blitsen.default` channel, submission, update, close,
body taps and action buttons through `android-activity` and `jni`. Android urgency is a builder hint
inside the user-controlled default channel, and `appName` cannot rename a channel Android has
already created. The APK's only dex provides a private activation receiver. It persists body,
action and delete Intents before body/actions launch the platform `NativeActivity` with no trusted
extras; a swipe dismissal does not open the Activity. An activation is delivered on the next
eligible frame, or once after a later launch if no session was alive.

### Cold-start activation

A notification outlives the process that showed it, so a click on one belonging to an application
that has exited is a launch rather than an event. What arrives is an `activation`:

```js
notify.onEvent?.(event => {
  if (event.type !== "activation") return;
  // `action` is null for a body click; a persisted dismissal carries
  // `reason: "dismissed"`. `id` names a notification from the session that
  // showed it, which is a session that has ended.
  resumeFrom(event.id, event.action);
});
```

It is delivered **once**, on the first frame turn, which is after the document's scripts have run—so
a listener registered at the top level of a module receives it. A reload does not repeat it, and
neither does a later launch: the activation is recorded as delivered in the application's own data
directory, keyed by a nonce the platform entry point minted, and an envelope offered a second time
is dropped. That is the same guard that keeps an Android `Intent` re-delivered to a recreated
Activity from arriving twice.

Shown notifications are not withdrawn merely because the application exits normally; shutdown
detaches this process's callback state and leaves the platform-owned notification for its registered
entry point. Reload is different: it replaces a live JavaScript session and closes that session's
notifications before installing the next one.

The identity that record is kept under is the one `blitsen build --bundle-id <id>` registered; on
Android it is the application ID the package was installed as, which the runtime reads from the
Activity. A development run has neither, and is told so if a platform hands it an activation:
notifications it shows can only be acted on while it is still running. Which entry points a platform
actually starts is [Platform support](PLATFORM-SUPPORT.md#notifications).

Browser-oriented integrations can use the standard `Notification` global over this same backend on
Linux, Windows, any macOS process that has a bundle identity—an exported application, or a
development run inside `--dev-bundle`—and any Android package the platform launched, where a body
tap has an application identity to come back to. See [Web API
support](WEB-APIS.md#notifications).

## Native input snapshots

DOM keyboard, pointer and wheel events remain the normal input path. `input.snapshot()` complements
them for frame-oriented applications with held physical keys and buttons plus raw mouse movement.

```js
import input from "blitsen/input";

function frame() {
  const state = input.snapshot?.();
  if (state?.keys.some(key => key.code === "ArrowLeft")) moveLeft();
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
```

Losing focus clears held keys and buttons. Movement and wheel fields are accumulated since the
previous snapshot and consumed by reading it. Ordinary pointer and keyboard events are unaffected.

The snapshot describes one pointer, in the same CSS pixels a `pointerdown` listener sees. Its
position is `null` whenever no pointer is in the window — before one arrives, after the cursor
leaves, and between taps on a touchscreen. Desktop and Android take the same snapshot, but not
every field is populated on both: Android reports position and a primary button for the finger
that is down, while raw `movementX`/`movementY` and the wheel fields are mouse signals it does not
produce. A second finger does not disturb the pointer the first one set; multi-touch is the DOM
pointer events, which carry every contact with its own `pointerId`. Keys are held by physical
code, so an Android soft keyboard — which reports characters without the key behind them — appears
as DOM composition and `input` events rather than here. Hardware keys still produce `keydown`.

Desktop targets also expose `input.onDeviceChange` and `input.vibrateGamepad`. The former is the
native convenience form of the standard `gamepadconnected`/`gamepaddisconnected` events and returns
an unsubscribe function. The latter addresses the stable index from `navigator.getGamepads()` and
starts dual-rumble with `duration`, `strongMagnitude` and `weakMagnitude`; it rejects if the slot
disconnected, has no actuator, or the driver refused the effect. There is deliberately no
`input.gamepads`: the standard snapshot already carries every controller field, and a second
registry could disagree with it. Android leaves both native members and the standard Gamepad API
absent because the controller backend has no Android implementation.

## Raw HID devices

`blitsen/hid` exchanges raw input, output and feature reports with devices that are not ordinary
input: instrument panels, DIY controllers, label printers, keyboard firmware configuration
interfaces. Keyboards, pointers and game controllers are not this module's — they are DOM events
and the standard Gamepad API.

```js
import hid from "blitsen/hid";

const found = (await hid.devices?.() ?? []).find(device => device.vendorId === 0x16c0);
if (found) {
  const device = await hid.open(found.id);
  device.onInputReport(report => {
    console.log(report.reportId, report.data);
  });
  // The first byte is the report ID, or zero for a device with none.
  await device.write(new Uint8Array([0x02, 0x01]));
  const settings = await device.receiveFeatureReport(0x03);
  console.log(settings.byteLength, device.maxFeatureReportSize);
  await device.close();
}
```

Enumeration is not permission to open anything. Blitsen refuses the Generic Desktop keyboard,
keypad, mouse and pointer collections, and it refuses the whole physical device when any of its
collections is one of those — on Linux a single hidraw node carries every collection, so opening a
keyboard's vendor interface would hand over its keystrokes as well. There is no way to opt out.
A device's report descriptor is re-checked once it is open, so a composite device whose enumeration
did not mention a keyboard is still rejected before a report is read.

A device's `id` is opaque and stable only for the process that issued it. It is not a device path
and not a serial number; `serialNumber` is reported as metadata where the device supplies one, and
is never the identity Blitsen opens by. `open()` rejects with a `DOMException` whose `name`
separates the outcomes an application has to handle differently: `NotAllowedError` for permission,
`NotFoundError` for a device that has gone, `NotSupportedError` for a collection Blitsen refuses,
and `OperationError` for a backend failure.

Reports are read on a native worker that owns the handle, and reach the application only at the top
of a frame, in order. An input report carries its report ID separately and its data without that
leading byte, so nothing depends on whether the platform retained it. `maxInputReportSize`,
`maxOutputReportSize` and `maxFeatureReportSize` come from the device's own report descriptor;
`write` and `sendFeatureReport` refuse a longer report before anything is sent. A device that
disconnects closes its handle and emits exactly one `onDisconnect` event; `close()` emits none.
`onDeviceChange` reports devices arriving and leaving, and is polled — no listener means no scan.

Access is a packaging question on every platform, and `blitsen doctor` reports it for the target
being built. Linux hidraw nodes belong to udev: `blitsen build` writes a `<name>.hid.rules`
template beside the executable for the distribution to complete and install, and Blitsen will
neither install one at run time nor suggest running as root. A sandboxed macOS application needs
`com.apple.security.device.usb`; `blitsen build` writes `<name>.app.entitlements` beside the bundle
for the signing command to pass to `codesign --entitlements`. macOS opens devices with shared
access, so Blitsen never takes a device away from the rest of the system. Windows reserves some
top-level collections for itself, which no packaging step unlocks.

Android has the same module over `UsbManager`, and one call behaves differently there: the grant is
not a packaging step but a system dialog, raised by the first `open()` of a device and answered by
the person using the application. That `open()` does not settle until they answer — it resolves on
a grant and rejects with `NotAllowedError` on a dismissal, which an application may ask about
again — and the grant belongs to that one device and ends when it is unplugged. Enumeration needs
no permission, but it cannot read a report descriptor either, so `usagePage` and `usage` are `0`
there: select a device by `vendorId` and `productId`, and read its usages after it is open. A boot
keyboard or mouse interface is refused before opening, as its desktop counterpart is.

This Android path type-checks and its logic is covered by host tests, but it has not yet been
exercised against a real device; see `docs/PLATFORM-SUPPORT.md`.

## Dialogs

Dialog calls are asynchronous and return real filesystem paths:

```js
import dialog from "blitsen/dialog";

const path = await dialog.openFile?.({
  title: "Open a project",
  filters: [{ name: "JSON", extensions: ["json"] }],
});

if (path) {
  console.log(path);
}
```

File dialogs resolve to `null` when dismissed. In this release the dialog module is available on
Linux desktop targets and absent on macOS, Windows and Android. Feature detection is required even
when your current development platform supports it.

## Clipboard images

Clipboard images use 8-bit RGBA pixels:

```js
import clipboard from "blitsen/clipboard";

const image = clipboard.readImage?.();
if (image) {
  console.log(image.width, image.height, image.data);
}
```

On X11 and Wayland, clipboard contents written by an application may disappear when the process
exits unless the desktop runs a clipboard manager. macOS and Windows hand the data to the system.

## Dropped files, and dragging out

Dropping a file into the window is a DOM event rather than a module call: the standard drag events
carry `dataTransfer.paths`, an array of absolute filesystem paths instead of the browser's `File`
objects. [Web API support](WEB-APIS.md#dropped-files-are-paths) documents it.

Starting a drag *out* of the window has no counterpart. `blitsen/window.startFileDrag` is recorded
as absent in the generated matrix rather than implemented: a drag source is a platform object driven
from the thread that owns the window, and on Windows and macOS it runs a modal loop that does not
return until the drop — on the one thread Blitsen keeps free to paint.

## Application directories

Pass one safe path segment to the directory helpers:

```js
import app from "blitsen/app";

const dataDirectory = app.dataDir?.("MyApp");
```

The methods return a platform-appropriate path but do not create it. Use your filesystem library
to create the directory before writing. `requestSingleInstanceLock` uses a private per-user Unix
socket or Windows named pipe and has the same invocation hand-off contract on every desktop.

## OS readings

`blitsen/os` reads CPU, memory, mounted storage, host identity, power and locale:

```js
import os from "blitsen/os";

const memory = os.memory?.();
if (memory) {
  console.log(`${memory.available} of ${memory.total} bytes available`);
}
```

Every call samples current state. Discard the first `os.cpu()` usage reading and use later calls to
measure the interval between samples.

`os.batteries()` lists the batteries the machine runs on. An empty list is the answer a desktop
gives rather than a refusal to answer, so test the length instead of the member:

```js
const [battery] = os.batteries?.() ?? [];
if (battery && battery.state === "discharging" && battery.level < 0.2) reduceFrameRate();
```

A machine that cannot be asked about power throws, which is what keeps that case distinct from a
machine that has none. Peripheral batteries — a wireless mouse, a keyboard — are not in the list.
The member is absent on Android, whose power service is a different API with its own semantics.

`os.locale()` reports the language tag and IANA time zone this session is configured for. Both are
values to hand straight to a formatter, and they are the same ones `Intl.NumberFormat` and
`Intl.DateTimeFormat` default to, so there is no second source of truth to keep in step.

Two capabilities are deliberately not on this module. Displays are `window.monitors()`, which
already reports every monitor's size, position, scale factor and refresh rate; a second list here
could disagree with that one. Idle time — seconds since the user last touched anything — is absent
on every platform rather than on the ones that cannot answer: Wayland has no answer at all for an
unfocused client, and reporting zero there is indistinguishable from a machine in use. It is also
the one reading that describes the person rather than the machine, so a partial implementation
would buy that signal on three platforms in exchange for a wrong answer on the fourth.

## Using the `native:*` spelling

The shorter `native:window` form only works when the bundler leaves it external. Prefer
`blitsen/window` unless you have a reason to expose runtime imports directly.

Vite and Rollup:

```js
import { blitsenVite } from "blitsen/bundler";

export default {
  plugins: [blitsenVite()],
};
```

The package also exports `blitsenRollup`, `blitsenEsbuild` and `blitsenWebpackExternals`. webpack
must emit ESM when it leaves `native:*` external.

## TypeScript

Installing `blitsen` provides native-module types and Blitsen's supported DOM additions. The
runtime surface is narrower than the browser's `lib.dom.d.ts`, so TypeScript cannot prove overall
compatibility. Continue to run `blitsen doctor` against built output.

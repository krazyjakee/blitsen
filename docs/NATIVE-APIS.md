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
| `blitsen/notify` | `show`, `permission`, `requestPermission`, `update`, `close`, `onEvent` |
| `blitsen/input` | `snapshot` |
| `blitsen/os` | `cpu`, `memory`, `storage`, `host`, `locale` |

The declaration files installed with `blitsen` document parameters and result types. The
[generated native module matrix](COMPATIBILITY.md#native-modules) is available when you need the
exact per-member runtime manifest.

## Window lifetime

Window and dialog methods need the native window, which exists from the `load` event onward:

```js
import windowApi from "blitsen/window";

addEventListener("load", () => {
  windowApi.setFullscreen?.(true);
});
```

Calling these methods from a document script before the window exists throws instead of silently
doing nothing.

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
Tray support is desktop-only. A native application menu is separate work because it must exist
without a tray icon (#249).

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
and otherwise reads the platform result. Linux returns `"granted"` because the freedesktop service
has no per-app authorization state—an unavailable service still makes `show` reject. macOS
requires an exported, signed `.app` bundle for authorization and uses its application icon;
passing `icon` is rejected. Linux accepts an icon theme name or absolute path. Windows accepts an
image path. Android accepts an application drawable resource name and otherwise uses a system
fallback icon.

IDs start again in each application session. Once a notification is clicked, acted on, dismissed,
expired or closed, `update` and `close` return `false`. Events are FIFO and are never delivered from
a platform callback thread. At most eight action buttons may be requested; the desktop is still
free to show fewer.

The `notify-rust` Windows backend delivers click, action, dismissal and error events, but does not
retain a toast handle for general replacement or close. Calls for an active ID therefore reject on
Windows instead of pretending they worked (#251). Windows also reports permission as `"default"`
until that backend exposes the notifier setting. Linux and macOS implement update and close.
Activation while the process is already running is delivered on desktop; launching a stopped
application from a notification requires platform registration and packaging work tracked in #252.
Android implements permission, an idempotent `blitsen.default` channel, submission, update and close
through `android-activity` and `jni`. Android action buttons and tap/dismiss lifecycle events remain
unavailable until #252 supplies an intent entry point; requesting actions therefore rejects rather
than displaying inert controls. Android urgency is a builder hint inside the user-controlled
default channel, and `appName` cannot rename a channel Android has already created.

Browser-oriented integrations can use the standard `Notification` global over this same backend on
Linux and eligible packaged macOS apps. It deliberately remains absent where its lifecycle contract
cannot be implemented, including Android until notification intent routing lands; see [Web API
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
previous snapshot and consumed by reading it. Gamepads, vibration and device-change events remain
absent; ordinary pointer and keyboard events are unaffected.

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
to create the directory before writing. `requestSingleInstanceLock` is currently Unix-only.

## OS readings

`blitsen/os` reads CPU, memory, mounted storage and host identity:

```js
import os from "blitsen/os";

const memory = os.memory?.();
if (memory) {
  console.log(`${memory.available} of ${memory.total} bytes available`);
}
```

Every call samples current state. Discard the first `os.cpu()` usage reading and use later calls to
measure the interval between samples.

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

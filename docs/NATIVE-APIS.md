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
| `blitsen/window` | `setSize`, `setFullscreen`, `isFullscreen`, `setDecorations`, `isDecorated`, `setAlwaysOnTop`, `setCursor`, `setCursorVisible`, `setCursorGrab`, `monitors` |
| `blitsen/dialog` | `openFile`, `openFiles`, `saveFile`, `openFolder`, `openFolders`, `message` |
| `blitsen/clipboard` | `readText`, `readHtml`, `readImage`, `writeText`, `writeHtml`, `writeImage`, `clear` |
| `blitsen/os` | `cpu`, `memory`, `storage`, `host` |

`blitsen/tray`, `blitsen/notify` and `blitsen/input` resolve but have no implemented members in this
release.

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

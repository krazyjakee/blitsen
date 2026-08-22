# Web API support

Blitsen implements the browser APIs needed by its supported application profile, not a complete
browser. Build-time checks and runtime feature detection are both part of using it safely.

## Check an application

Run doctor against built output:

```sh
npx blitsen doctor dist
```

Errors block export because the scanner found a construct the application cannot recover from.
Warnings identify missing or narrower behavior that may be guarded by a fallback—or may fail when
the path executes. Review every warning and test the result in Blitsen.

For a machine-readable report or a different target:

```sh
npx blitsen doctor dist --json
npx blitsen doctor dist --target win32-x64
```

## Supported areas

This is a practical summary. The [generated compatibility matrix](COMPATIBILITY.md#capability-tiers)
lists individual globals, classes and members.

| Area | Current support |
| --- | --- |
| DOM | Documents, elements, text, fragments, templates, attributes, selectors, mutation observers and common traversal/mutation APIs |
| Events | Event targets, custom, mouse, keyboard, focus, input, pointer, wheel, error and submit events |
| Forms | Basic input, textarea, select, option, button and form state; keyboard editing and selection |
| Layout reads | Bounding rectangles, client/offset geometry, computed style, scrolling, ranges, carets and selection |
| Scheduling | `requestAnimationFrame`, timeouts and intervals |
| Networking | Buffered `fetch`, request/response/headers/blob, abort signals, WebSocket and `EventSource` |
| Workers | Dedicated workers, message channels, structured clone and transferable buffers |
| Routing | `location`, `history`, hash changes and popstate within the application |
| Styling | Stylesheets, rule source, media queries, CSS support checks and resize observers |
| Audio | `<audio>` and a focused Web Audio subset |
| Storage | `localStorage` and `sessionStorage`, both in-memory for one process |
| Canvas | `<canvas>` with a full 2D context: paths, text, images, gradients, patterns, compositing, `getImageData` and `toDataURL` |

## Important absences

| Feature | What to use or expect |
| --- | --- |
| WebGL and WebGPU | Not implemented; `getContext("webgl")` answers `null`. Use the 2D context, or `<blitsen-view>` for GPU output |
| Canvas shadows and `ctx.filter` | Absent, so a feature test selects a fallback; both need a blur the renderer has none of |
| `OffscreenCanvas` and `ImageBitmap` | Absent; a `<canvas>` that is never in the document draws, reads back and encodes |
| WebAssembly | Absent from the standard shipped JavaScript engine |
| XHR | Use `fetch` |
| Streams | Responses are buffered; streaming body APIs are absent |
| FormData, File and FileReader | Absent; use supported request bodies or native file paths |
| IndexedDB | Absent; use application-owned durable storage |
| SharedWorker and ServiceWorker | Absent; dedicated `Worker` is supported |
| Browser modal dialogs | `alert`, `confirm`, `prompt` and `print` are absent; use `blitsen/dialog` where available |
| Cookies | No cookie jar; `document.cookie` is absent |
| Custom elements and shadow DOM | Absent; `DOMParser` is supported |
| Video and text tracks | Absent; audio is supported |
| Accessibility tree | Not exported to the platform in this release |
| Full IME and complex text editing | Incomplete; verify every input language and workflow you support |

## Feature detection

Missing APIs are absent rather than installed as no-op stubs:

```js
if ("ResizeObserver" in globalThis) {
  const observer = new ResizeObserver(handleResize);
  observer.observe(element);
}
```

The same rule applies to optional native members:

```js
import dialog from "blitsen/dialog";

if (dialog.openFile) {
  const path = await dialog.openFile();
}
```

Do not infer support from TypeScript's browser library. A package can add Blitsen declarations but
cannot remove unsupported names from `lib.dom.d.ts`; doctor checks the built application instead.

## Internationalisation

`Intl` is implemented natively over CLDR, so every operation below is the locale's own data rather
than an approximation of it, and there is no locale list to declare — the whole of CLDR ships.

| Implemented | Absent |
| --- | --- |
| `Intl.NumberFormat`: decimal, percent, currency (minor units from the currency), compact notation | `formatToParts` and `formatRange`, on every formatter |
| `Intl.DateTimeFormat`, including **named IANA `timeZone` values** and their daylight-saving history | `Intl.Segmenter` |
| `Intl.RelativeTimeFormat`, `Intl.PluralRules`, `Intl.Collator`, `Intl.ListFormat` | `Intl.DisplayNames`, `Intl.DurationFormat`, `Intl.supportedValuesOf` |
| `toLocaleString`, `toLocaleDateString`, `toLocaleTimeString` and `localeCompare`, over those formatters | — |

Two operations have no fallback an application can write for itself, and are the reason this is
native rather than documented away: **converting an instant into a named time zone**, which needs
the zone's history of offsets, and **formatting a currency**, which needs the placement and minor
units CLDR gives that code in that locale. `os.locale()` reports the tag and zone the session is
configured for. The details and the deviations are in
[COMPATIBILITY.md](COMPATIBILITY.md#intl).

## Renderer differences

HTML and CSS are rendered by Blitz rather than a browser engine. Some valid browser styles render
differently or are ignored. Current high-impact areas include transitions, fixed/sticky positioning,
paint effects, form-control styling, font fallback and complex text.

**SVG paints** — inline `<svg>`, `<img src="icon.svg">` and CSS `background-image`, as vectors
rather than as rasterised images. Shapes, paths, `viewBox`, transforms, gradients, dashed strokes,
`currentColor` and `<text>` all render; `filter`, `mask`, SMIL animation and `<pattern>` fills do
not, and a `<pattern>` fill additionally leaves a red mark in the frame's corner. `doctor` reports
those constructs specifically rather than warning about every `<svg>`. SVG `<text>` resolves fonts
through a different database from HTML text and can silently find none — see
[COMPATIBILITY.md](COMPATIBILITY.md#svg) before putting chart labels inside an `<svg>`.

Doctor reports patterns it can recognize, but it cannot prove visual equivalence. Keep screenshot or
interaction tests for important layouts and verify them on each target operating system.

## Local and remote resources

An export has no web server. Local HTML, CSS, modules and assets are loaded from the application
bundle. Remote `fetch` and WebSocket are supported; remote script/module loading and remote
subresources are deliberately narrower or refused. Prefer a self-contained application and use
relative local URLs.

## Security model

A Blitsen application is trusted native software. There is no same-origin policy, browser sandbox,
permission prompt or safe boundary for untrusted third-party pages. Validate remote data as you
would in any native application and never use the runtime as a general web-content viewer.

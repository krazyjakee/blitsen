# v0 compatibility profile

Blitsen v0 accepts built static applications that stay within the surface below. The profile is
deliberately narrower than “works in a browser”: it describes what the current runtime and Blitz
renderer can support consistently enough to make an adoption claim.

Run the check against build output, not source:

```sh
npx blitsen doctor dist
npx blitsen doctor dist --json
```

`doctor` exits non-zero for profile errors. A build repeats every diagnostic and **fails on any
error** — an export that cannot run is not worth producing — while warnings let feature-detected
fallback paths through. The scan is static and conservative: it finds references, not only
executed paths, which is why [severity](#diagnostic-severity) is narrow. Every rule it applies to
JavaScript comes from the [generated manifest](#capability-tiers) below.

## Strict v0 surface

| Area | In profile |
| --- | --- |
| Application shape | One built `index.html` plus the local files reachable from it; root-relative HTML/CSS asset URLs are normalized while ingesting without changing `dist` |
| JavaScript | ES modules already emitted by the application's bundler |
| Framework DOM | Stable node identity, standard node type/name/value/owner fields, `MutationObserver`, creation/insertion/removal, text and attributes, elements, comments, namespaced elements, fragments and `<template>` |
| Selection and collections | `querySelector`, `querySelectorAll`, `getElementsByTagName` and `getElementsByClassName` on the document and on an element, `getElementById`, `closest`, `matches`, `children` and the element-traversal properties, `dataset`, `attributes`, static `NodeList`, `classList`, `link.relList` |
| Events | Capture/target/bubble listeners, click, mouse, wheel, keyboard, focus, resize and lifecycle events |
| Style read-back | `getComputedStyle`, `matchMedia`/`MediaQueryList`, `ResizeObserver`, `CSS.escape`/`CSS.supports` |
| Geometry and text | `getBoundingClientRect`, `getClientRects`, the offset/client/scroll box properties, `clientTop`/`clientLeft`, `offsetParent`, `innerText`, `compareDocumentPosition`, `elementFromPoint` |
| Scrolling | `window.scrollTo`/`scrollBy`/`scroll`, `scrollX`/`scrollY`/`pageXOffset`/`pageYOffset`, `element.scrollTop`/`scrollLeft`, `scrollIntoView` |
| Parsing | `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `insertAdjacentElement`, and `DOMParser` for `text/html` into a fragment |
| Scheduling | `requestAnimationFrame`, timers and microtasks |
| Networking | `fetch`, `Headers`, `Request`, `Response`, `Blob`, `AbortController` over `http`/`https`, with buffered bodies |
| Audio | Web Audio — a context, gain, stereo panning and buffer sources over decoded files — and `<audio>`/`new Audio()` for whole-file playback |
| Routing | In-memory `history` and `location`, `popstate` and `hashchange` |
| CSS | Static block, flex and grid layout; bounded absolute positioning; spacing, borders, backgrounds, colors and system typography |
| Subresources | `<img>` and CSS `background-image` (PNG, JPEG, GIF, WebP), and `@font-face` web fonts (WOFF2, WOFF, TTF, OTF), loaded from local files; SVG images and `<video>` are not. Audio is loaded and decoded by Web Audio rather than as a renderer subresource — see [Audio](#audio). A subresource the export cannot serve — a remote URL, or a local file that is missing — is answered with an empty body, so the document paints without it rather than waiting on it |

The M3b acceptance app intentionally uses the normal Vite default output, including
root-relative `/assets/...` references and Vite's module-preload bootstrap. It contains no
Blitsen imports or runtime branches.

## Asset URLs

There is no web server behind an exported application, so a URL that assumes a server root has to
be resolved at build time. **Blitsen rewrites server-root URLs while ingesting, in its own staging
copy — your `dist` directory is never modified.**

| You wrote | Blitsen does |
| --- | --- |
| `href="./assets/app.css"` | Nothing; relative URLs already work. |
| `src="/assets/app.js"` (default `base`) | Rewrites to the equivalent document-relative path. |
| `src="/app/assets/app.js"` (custom `base: "/app/"`) | Drops the base prefix that does not exist in the output, then rewrites. |
| `url("/assets/hero.png")` in CSS, and `@import` | Same rewrite, applied transitively. |
| `<a href="/settings">` | Nothing; anchors are navigation, not subresources. |
| `href="https://cdn…"` or `//cdn…` on a subresource | Warns. The request is answered with nothing, so the page renders without that stylesheet, font or image. |
| `<script src="https://cdn…">` | Warns. The loader skips that one script and says so on stderr; every other script on the page still runs. |

**Only HTML and CSS are rewritten.** JavaScript is left byte-identical, because a path assembled
at runtime cannot be safely edited by a regular expression. In practice:

- `new URL('./x.png', import.meta.url)` and a relative `import()` **work** — the export preserves
  your directory layout, so relative resolution lands on the same file it did on a server.
- `new URL('/assets/x.png', …)` and any specifier built from a variable or template literal **do
  not work**, and are not diagnosed. Configure your bundler with a relative base (Vite:
  `base: './'`) if your application computes asset URLs from a server root.
- `fetch('/data.json')` does not work either — see [Networking](#networking) — but a literal URL
  is diagnosed as an error.

## Networking

`fetch`, `Headers`, `Request`, `Response`, `Blob`, `AbortController` and `AbortSignal` are
Blitsen's own, backed by `reqwest`. They are not the host's: the Phase 1 Bun globals are replaced
so that the Phase 2 engine swap changes nothing an application can observe.

**There is no same-origin policy and no CORS, and this is deliberate.** An exported application
is trusted native software that happens to be written in HTML, not a document downloaded from a
site, so there is no origin to protect it from and no server to ask for permission. A request
goes where the application sends it. `mode`, `credentials`, `integrity` and `referrerPolicy`
describe a policy Blitsen does not have; they are not exposed on `Request` and passing them to
`fetch` changes nothing.

Requests run on a worker pool, never on the thread that owns the DOM. **Results land at one
defined point in the frame turn** — the start of the animation-frame stage, before any
`requestAnimationFrame` callback of that turn — so a response can never arrive in the middle of
one. The promise reactions themselves run at the microtask checkpoint that ends the turn, which
means a handler that mutates the DOM is painted by the following frame.

**Streaming bodies are not implemented, and will not be in v1.** `fetch` buffers a whole
response. `Response.prototype.body`, `ReadableStream` and `Response.clone` are *absent*, not
null-valued, so `if (response.body)` selects a buffered fallback correctly. The reason is
coherence rather than difficulty: WHATWG streams are a large surface Blitsen does not otherwise
provide, and exposing the host's would reintroduce exactly the Phase 1/Phase 2 divergence this
API exists to remove. A per-chunk delivery path also has no defined place in the frame turn
above, which is the contract the rest of the runtime is built on. Revisit when an application
needs a download progress bar or a long-lived response, not before.

| You wrote | What happens |
| --- | --- |
| `fetch("https://api…")` | Runs off-thread; resolves at the next frame turn. |
| `fetch("/api/data")` | Fails. There is no server behind the document address; `doctor` reports it as an error. |
| `new Request(…)`, `new Headers(…)`, `new Response(…)` | Full subset above, including `AbortSignal`. |
| `response.text/json/arrayBuffer/blob()` | Supported; a body is readable once. |
| `response.body`, `response.clone()`, `FormData` bodies | Absent. |
| `window.stop()` | Aborts the load in progress; see below. |

**`window.stop()` aborts loading, and only loading.** Every outstanding `fetch` rejects with an
`AbortError` — the rejection its own `AbortSignal` would have produced — and every subresource the
renderer is still waiting on is cancelled *and settled*, never merely abandoned: a request left
pending would block painting for the life of the document, which is the opposite of what a caller
asking to stop loading wants. Timers and animation frames keep running, as they do in a browser;
they are the application's own work, not the document's load. There is no parser half either — a
Blitsen document is parsed whole before any of its scripts run. A request made afterwards loads
normally, because `stop()` ends the load in progress rather than the document's ability to load.
With nothing in flight it does nothing observable, which is not the same as being a function that
does nothing: both halves run and find nothing to abort.

### WebSocket

`WebSocket` is Blitsen's own, backed by `tokio-tungstenite`, and is the streaming path this
runtime does have. The constructor, `url`, `readyState` and its four constants, `protocol`,
`extensions`, `bufferedAmount`, `binaryType`, `send` and `close` are all present, as are the
`open`, `message`, `error` and `close` events; a close carries its code, reason and `wasClean`.

Text and binary frames both work. `binaryType` is `"blob"` by default and `"arraybuffer"` when
asked, and the choice is made once at the boundary rather than by converting afterwards. `send`
accepts a string, a `Blob`, an `ArrayBuffer` or a typed array, and throws `InvalidStateError`
before the socket is open — which is the one thing about a socket that is not a queued no-op.

`wss://` uses the platform certificate store, through the same `native-tls` backend `fetch`
resolves to, so a certificate the desktop trusts is one the socket trusts.

The connection runs off the thread that owns the DOM, and **frames land at the same defined point
in the frame turn that `fetch` results do**. An open socket keeps the host turning, for the same
reason an in-flight request does: its landing point is that turn, so a loop that idled would never
deliver. A non-`ws:`/`wss:` address is refused with a `SyntaxError` at construction.

`EventSource` is absent. Feature-detect it, or hold the stream open over a socket instead.

## Audio

Backed by [`web-audio-api`](https://crates.io/crates/web-audio-api), which is a Rust
implementation of the Web Audio API itself rather than a playback library with a graph rebuilt on
top of it — so what an application schedules is what the specification says it scheduled. Decoding
is Symphonia; output is `cpal` (WASAPI, CoreAudio, ALSA).

**Nothing opens the sound card until an application asks it to.** The context is created on the
first `new AudioContext()`, so an application that never plays a sound never touches the device.

**A machine with no output device still runs.** If the device cannot be opened the context falls
back to a silent sink and says so once on stderr. An application then behaves exactly as it would
for a user who has muted their speakers, which is the same thing as far as its own code can tell —
rather than throwing from a constructor and taking down every page that plays a click.

### What is implemented

| | |
| --- | --- |
| Context | `AudioContext`, `sampleRate`, `currentTime`, `state`, `destination`, `resume`, `suspend`, `close` |
| Nodes | `GainNode`, `StereoPannerNode`, `AudioBufferSourceNode`, `AudioDestinationNode` |
| Parameters | `AudioParam` — `value`, `setValueAtTime`, `linearRampToValueAtTime`, `exponentialRampToValueAtTime`, `setTargetAtTime`, `cancelScheduledValues` |
| Buffers | `decodeAudioData` (promise and callback forms), `AudioBuffer`, `getChannelData` |
| Element | `Audio`, `<audio>`, `HTMLAudioElement` — `play`, `pause`, `currentTime`, `duration`, `volume`, `muted`, `loop`, `paused`, `ended`, `canPlayType` |

**Formats are whatever Symphonia decodes**, which is not selectable: AAC, ADPCM, ALAC, FLAC,
MP1/MP2/MP3, PCM and Vorbis, in AIFF, CAF, ISO/MP4, MKV/WebM, Ogg, WAV and raw containers.
`canPlayType` answers `"probably"` or `""` and never `"maybe"`, because a maybe tells a caller
nothing.

**Decoding runs on the worker pool** and lands at the same point in the frame turn `fetch` results
do. A decode in flight keeps the host turning, so its result cannot be stranded.

A source plays **once** — the specification says so, and starting one twice throws
`InvalidStateError`. Overlapping playback of one sound is several sources over one decoded buffer,
which is also what makes it cheap: the decode is paid for once.

### What is absent, and why

**The rest of the Web Audio graph.** `BiquadFilterNode`, `OscillatorNode`, `AnalyserNode`,
`ConvolverNode`, `DelayNode`, `DynamicsCompressorNode`, `WaveShaperNode`, `PannerNode` and its HRTF
spatialisation, `ChannelSplitterNode`/`ChannelMergerNode`, `AudioWorklet`, `OfflineAudioContext`
and `AudioListener` are all things the backing crate implements and this bridge does not name.
That is deliberate: every API named here is a published claim that `doctor` and the capability
tiers make on Blitsen's behalf, and the surface above is what an application asking for sound
effects, cues and a background loop actually uses. They are cheap to add when something measured
asks for one.

**`<audio>` is not a streaming media element.** The source is fetched whole and decoded whole
before playback starts, which is right for the sounds a desktop application has and wrong for an
hour of audio. `buffered`, `seekable`, `readyState`, `networkState`, `preload`, `played` and
`HTMLMediaElement` itself are absent rather than answered with a fiction. `<video>` is not
implemented at all.

**`webkitAudioContext`** is absent: it is a prefix for a browser this is not.

### Testing audio

`BLITSEN_AUDIO_OFFLINE=1` makes the context an offline one that renders to sample buffers with no
device at all. That is how Blitsen's own harness asserts on audio — reading the samples that came
out, the same way the renderer's tests read painted pixels — rather than on the calls that were
made. A graph built correctly that rendered silence would pass any check that only read properties
back.

## Routing

`history` and `location` exist and are **in memory only**. There is no navigation, no network and
no back-forward cache — the document is never left, so nothing to navigate to and nothing to
restore. This is the surface a client-side router needs (React Router, Vue Router and
equivalents), and it is deliberately not more than that.

The document address is `blitsen://app/`. It is synthetic because an exported application has no
server and therefore no origin, and it is path-rooted because that is what a router reads. The
scheme makes the absence of an HTTP origin visible rather than pretending to be `localhost`.

| Supported | Absent |
| --- | --- |
| `location.href/protocol/host/hostname/port/origin/pathname/search/hash` | `location.assign/replace/reload`, `ancestorOrigins` |
| `location.hash = …` (pushes an entry, fires `hashchange`) | Assigning `href`, `pathname` or `search` — refused with a `NotSupportedError` naming `pushState`, never silently |
| `history.pushState/replaceState/go/back/forward`, `length`, `state`, `scrollRestoration` | Cross-origin entries — refused with a `SecurityError`, as in a browser |
| `popstate` and `hashchange` on `window`, `PopStateEvent`, `HashChangeEvent`, `document.location` | `navigation` (the Navigation API) |

Two differences from a browser worth knowing. `history.state` holds the value you pushed rather
than a structured clone of it, so mutating that object mutates the entry. And `scrollRestoration`
is recorded and reported but restores nothing, because a traversal never reloads a document.

An anchor still does nothing: `<a href="/settings">` is navigation, and a router that calls
`preventDefault` and `pushState` is what makes it work — which is what every client-side router
already does.

Checked against the real libraries rather than a reading of their source: React Router 7
(`createBrowserRouter` and `createHashRouter`, including `navigate(-1)` traversal through
`popstate`) and Vue Router 4 (`createWebHistory` and `createWebHashHistory`, including
`router.back()`) resolve, match and traverse routes unmodified on this surface.

## Nodes, fragments and templates

The HTML parser makes node kinds `createElement` cannot, and framework runtimes need every one of
them: `createComment` for Vue's `v-if` and fragment anchors, `createElementNS` for inline SVG,
`createDocumentFragment` and `<template>.content` for Svelte 5's cloned templates. These are real
nodes in the renderer's tree, not JavaScript stand-ins.

Three differences from a browser are worth knowing:

- **Collections are static.** `children`, `querySelectorAll`, `getElementsByTagName`,
  `getElementsByClassName` and `attributes` return a snapshot rather than a live collection. A
  re-query sees a mutation; the collection handed out before it does not. The `Attr` objects in
  `attributes` are the exception — each still reads and writes through its element.
- **A fragment is a detached `<template>` element underneath**, which is what gives its children a
  real parent to be parsed, serialized and cloned against — including table rows, which any other
  parsing context would discard. `cloneNode(true)` copies by serializing and reparsing, so a clone
  carries the tree and its attributes and nothing else: no listeners and no JavaScript state, which
  is what the DOM specifies anyway.
- **`template.content` takes the parsed children off the element** the first time it is read,
  because Blitz has no separate template-contents document. The element is empty afterwards, which
  is what the specification says it was all along.

A comment's data is fixed when it is created, and data that would close the comment early
(`-->`) is refused rather than silently truncated. `attachShadow` remains absent, as does
`document.currentScript`: nothing in the bridge is told which script element is executing.

`setAttributeNS`, `getAttributeNS` and `removeAttributeNS` key an attribute by namespace and
local name, which is the pair they ask for — so `xlink:href` round-trips and `getAttribute`
correctly does not see it. The prefix itself is not stored: `getAttributeNames()` reports `href`
and serialization writes `href="…"`, which is already true of markup the parser read.
`getClientRects` returns the one border box `getBoundingClientRect` does, off the same layout
flush, because Blitz lays an element out as a single box with no fragmentation to report.

`link.relList` exists chiefly so that `relList.supports("modulepreload")` can answer truthfully.
Without it Vite's own module-preload polyfill installs itself and `fetch`es every chunk over an
address with no server behind it, which takes down any code-split build. The preload keywords are
honoured by doing nothing: an exported application's chunks are local files with no cache to warm.

## Reading style back

Blitz has already resolved the cascade, evaluated `@media` and laid the document out. These three
APIs ask it those answers from JavaScript rather than keeping a second idea of what an element's
style is; none of them is a shadow implementation.

**`getComputedStyle(element)`** returns a read-only `CSSStyleDeclaration` over the resolved
style — the stylesheet, not the inline declaration `element.style` reads. It is live: the same
object reflects a class or attribute mutation on the next read. Custom properties resolve through
inheritance, so a `--brand` declared on `:root` reads on any descendant.

Every read is layout-dependent, because CSSOM resolves `width` and `height` to the **used** value:
`width: 50%` reads as the pixel width layout produced. So a read goes through the same layout flush
`getBoundingClientRect` does, and a read *after* a write is a forced synchronous layout — the
expensive kind, counted by `BLITSEN_DEV_LAYOUT_WARNINGS` alongside the geometry reads. Batch reads
before writes as you would in a browser.

Where it differs from a browser:

- **An element the cascade has never reached reads empty.** A detached element is not in the
  document, so this renderer has no resolved style for it at all, and every property reads `""`
  rather than the initial value a browser would invent. Everything connected — including
  `display: none` subtrees — resolves normally.
- **Shorthands serialize from their longhands**, and read `""` when those longhands do not compose
  into one — which is also what `all` does in a browser. `margin`, `padding`, `border-width` and
  the rest of the ordinary shorthands compose.
- **The declaration is not enumerable.** `length`, `item()` and index access are absent; ask for
  the properties you want by name. `cssText` is `""`, which is what CSSOM specifies for a computed
  block anyway, and every mutator throws `NoModificationAllowedError` rather than silently
  ignoring the write.
- **Pseudo-elements are refused**, with a `NotSupportedError` naming the selector. A pseudo-element
  box is not addressable here, and answering with the originating element's style would be a wrong
  answer rather than a missing one.
- Only `width` and `height` are used values. The inset and box properties report their computed
  value, which is the declaration resolved to absolute units rather than the used geometry.

**`matchMedia(query)`** runs the query through the same parser and the same evaluator the cascade
uses for `@media`, so what matches in a stylesheet matches here. `MediaQueryList` carries `media`,
`matches`, `addEventListener("change")`, `onchange`, and the pre-2019 `addListener`/`removeListener`
a library still installs; the event is a `MediaQueryListEvent` with `media` and `matches`.

The features the style engine implements are `width`, `height`, `device-width`, `device-height`,
`orientation`, `aspect-ratio`, `resolution`, `device-pixel-ratio`, `scan`, `pointer`, `any-pointer`,
`hover`, `any-hover` and `prefers-color-scheme`. Anything else — `prefers-reduced-motion`,
`prefers-contrast`, `forced-colors` — is an unknown feature to the engine, and an unknown feature
does not match, which is the CSS answer rather than a Blitsen one. An unparsable query serializes
as `not all` and does not match, as it does in a browser.

`prefers-color-scheme` is **`light` for the life of the process**: the window is created with a
light colour scheme and nothing changes it yet, so a dark-mode toggle driven by the system
preference stays light while one driven by a class or `localStorage` works normally. The only
device state that can change is the viewport, so a `change` event is dispatched when — and only
when — a window resize flips a query, at the start of the frame turn.

**`ResizeObserver`** observes elements, with `observe`, `unobserve` and `disconnect`. An entry
carries `target`, `contentRect`, `borderBoxSize` and `contentBoxSize`; `contentRect` is the content
box positioned from the border box's own origin, exactly as the specification defines it.

Observations are delivered **at the start of the frame turn**, beside the `<blitsen-view>` surface
resizes and before any `requestAnimationFrame` callback — the same defined point in the turn that
network results land at. The first observation for an element is guaranteed: an undelivered
observation keeps the host turning the way an in-flight request does. A browser delivers after
layout and before paint instead, so an entry here describes the layout the previous frame settled
on. `box: "device-pixel-content-box"` is refused with a `TypeError`, because the device-pixel
snapping it reports is not exposed; `inlineSize` is width and `blockSize` is height, which holds
for every writing mode this renderer lays out.

`IntersectionObserver` and `PerformanceObserver` remain absent.

## Rendered text, box reads and scrolling

The read-back surface a component library reaches for once it starts measuring rather than only
rendering. Each of these asks Blitz the question rather than keeping a second answer beside it.

**`innerText`** is rendered text, which is the whole of what separates it from `textContent`: a
`display: none` or `visibility: hidden` subtree contributes nothing, a `<br>` is a line break, and
a block-level child starts a new line. What it does *not* do is re-derive line wrapping — it reads
the tree and its computed display rather than Blitz's line boxes, so a paragraph that wrapped over
three lines is one line here. Writing it is the inverse: newlines become `<br>` elements.

**`clientTop` and `clientLeft`** are the resolved border widths, read from the computed style
rather than differenced out of the border and content boxes — which would fold the padding in too.
**`offsetParent`** is the nearest positioned ancestor, or the body once the walk runs out, and is
`null` for an element the cascade is not laying out.

**`compareDocumentPosition`** returns the DOM's bitmask, computed by walking to the common
ancestor. Two nodes in different trees report `DISCONNECTED | PRECEDING |
IMPLEMENTATION_SPECIFIC`, as a browser does.

**`document.elementFromPoint`/`elementsFromPoint`** are the hit test the native window already runs
for every click, asked the other way round.

**Scrolling.** `window.scrollTo`, `scrollBy` and `scroll` move the document — `document.scrollingElement`,
the root element — and `scrollX`, `scrollY`, `pageXOffset` and `pageYOffset` read the offset back
live. `element.scrollIntoView` scrolls each scrolling ancestor and then the document until the
element's border box is inside each one, honouring `block` and `inline` including `nearest`.
`behavior` is accepted and ignored on both: there is no animation to run, so the scroll lands.

**`hidden`** reflects the content attribute, and the user-agent rule mapping `[hidden]` to
`display: none` applies as it does in a browser — including being overridden by an author rule that
sets `display` on the same element, which is ordinary cascade order rather than a Blitsen quirk.

`CSS.escape` and `CSS.supports` are present. `supports` answers from the cascade's own parser, by
round-tripping the declaration through an inline style, so it reports what *this* runtime accepts;
its one-argument form understands a plain `(property: value)` condition and answers `false` for a
compound one rather than guessing at it.

**`DOMParser` parses `text/html` into a detached fragment**, not into a second document — there is
one document in this runtime. `body` and `documentElement` are that fragment, and `head` is `null`,
because the fragment parser drops `<html>`, `<head>` and `<body>` tags and a parsed string is
therefore not split into head and body. An XML type is refused with a `TypeError` rather than run
through the HTML parser, which would mis-parse it silently.

### What is absent here, and why

- **`customElements` and `ShadowRoot`** — a decision, not an omission. Upgrading elements after
  parsing, running the lifecycle callbacks and ordering reactions is real machinery, and
  `<blitsen-view>` is registered natively rather than through a registry, so a user-defined element
  would need either its own registry beside that one or a merge of the two. Absent is also the
  shape a polyfill installs itself into.
- **`getSelection` and `Range`** — a large surface, and nothing measured has reached for it. They
  are absent together: a caller with a selection wants the ranges in it.
- **`document.currentScript`** — the script runner does not carry the script element through to
  evaluation, and a module script's `currentScript` is `null` in a browser anyway, which is what
  every bundler in the profile emits.
- **`document.doctype`** — the backend's tree has no doctype node to report.
- **`outerWidth`, `outerHeight`, `screenX`, `screenY`** — the platform layer exposes no window
  frame or position, and `innerWidth`/`innerHeight` already answer for the viewport. A second
  answer that could disagree with those is worse than no answer.
- **`visibilityState`, `execCommand`, `createRange`, `createTreeWalker`, `createNodeIterator`, and
  the `document.forms`/`images`/`links`/`scripts` collections** — each needs a reason to exist
  rather than a reason not to, and none has one yet.

## Stylesheets

**A stylesheet is the element that owns it.** `document.styleSheets` lists the `<style>` and
`<link rel="stylesheet">` elements the cascade is reading, in the order it applies them;
`styleElement.sheet` is the same object, one per element for the element's whole life, and
`sheet.ownerNode` is the element it came from. A disconnected element has no sheet, because
nothing it says has reached the cascade.

That identity is the whole design. A `<style>` element's text *is* its sheet's source: Blitz parses
it and hands it to Stylo, and reparses it whenever the text changes. So `insertRule` and
`deleteRule` rewrite that text, and the rule they insert is in the same stylesheet set the
document's own rules are in. There is no second rule list that could parse successfully and then
cascade nothing, which is the failure this API is easiest to build.

Two consequences worth knowing:

- **`cssRules` is derived from the source on every read.** The list object handed out is a frozen
  snapshot, like every collection here, but the next read sees what the last mutation did — so an
  index taken from `cssRules.length` and passed straight to `insertRule`, which is how a framework
  appends a rule, means what it means in a browser. A rule's `cssText` is its source text, not a
  reserialization, so rewriting a sheet to insert one rule cannot quietly rewrite the others.
- **A rule is inserted whole or refused.** Text that is not exactly one rule — to the cascade's own
  parser, not just structurally — throws a `SyntaxError`, and an out-of-range index throws an
  `IndexSizeError`. Nothing is written that the cascade would then ignore.
- **The element's text follows its rules.** A browser keeps `styleElement.textContent` at whatever
  was authored and holds the rule list separately; here they are the same thing, so a sheet mutated
  through `insertRule` reads back its rules as the element's text. Comments between rules are not
  rules and do not survive a mutation, which is also what a browser's rule list reports.

What is absent: the `CSSRule` subclasses and everything read off a rule other than its text
(`style`, `selectorText`, `type`), `disabled`, `replace`/`replaceSync`, constructible sheets
(`new CSSStyleSheet()` throws, so a feature test selects its fallback) and `adoptedStyleSheets`.
The rules of a sheet loaded from a URL are refused rather than reported as empty: that sheet's
source is a file this process fetched, not text in the tree. It is still listed in
`document.styleSheets` and still answers `ownerNode` and `href`.

### The animation clock

CSS animations and transitions are sampled at **the frame's own timestamp**, set from the same
value `requestAnimationFrame` callbacks receive, once per frame turn. Nothing below that reads a
clock of its own, which is what keeps a replayed or recorded frame sequence identical to the one
that was captured, and a running animation keeps the host turning the way an in-flight request
does.

The consequence is that **animation only advances on delivered frames**. A harness that loads a
document and never turns the loop sees every animation pinned to its first keyframe — which is
correct, not stalled: no frame has been asked for. This is also why a `@keyframes` rule inserted
from JavaScript is worth having at all; until the clock was wired to the frame it would have
parsed, cascaded, and never moved.

## Form controls

The whole of this surface rests on one distinction: **the content attribute is the control's
default, and the property is its current state.** `value` is not `getAttribute("value")`. Typing
into a field, or assigning to `value`, moves the state and leaves the attribute where it was —
HTML calls that the dirty value flag — and from then on the attribute is only the default. So
`defaultValue` and `defaultChecked` are the attribute reflections, `value` and `checked` are the
state, and each pair moves without the other. Getting this backwards would look like it worked,
which is why it is the thing the tests assert first.

There is one copy of that state and it is the renderer's. Blitz already keeps a text editor for
`<input>` and `<textarea>` and a checkedness flag for a checkbox, and those are what it paints
from; `value` and `checked` read and write exactly those, rather than a second store beside them
that could disagree with the pixels. Two consequences follow. A value assigned before the control
has ever been laid out is held until Blitz builds its editor and then pushed into it, so nothing is
lost by writing early. And a `<textarea>`'s child text — its default value, where an input has an
attribute — is given to the editor too, so an untouched textarea paints what it reads and tracks
its children the way HTML says a textarea with no dirty flag does.

`<select>` and `<option>` are the exception, because Blitz renders a `<select>` as its options
rather than as a control and has no notion of selectedness. An option's selectedness is stored as
the same flag a checkbox uses, which is the flag `:checked` matches against, so `select
:checked` finds the selected option the way a browser does — Svelte 3 reads a bound select exactly
that way. `select.value`, `selectedIndex`, `selectedOptions` and `option.index` are all derived
from the options, so there is nothing to keep in step.

`options` and `form.elements` are **snapshots**, like every other collection this runtime hands
out: a re-read sees an option added since, the collection handed out before it does not.

Two divergences worth knowing:

- **A drop-down with nothing selected reports its first enabled option** rather than `-1`. That is
  the selectedness HTML resets a drop-down to, and it keeps `select.value` meaningful; what it does
  not do is stay at `-1` after `selectedIndex = -1` or after assigning a value no option carries.
- **`:checked` does not restyle.** A selector query evaluates it against current state and finds
  the right element, but changing checkedness does not invalidate the cascade, so a `:checked` CSS
  rule will not repaint. This is Blitz's behaviour for its own checkboxes too.

### Submission

**`form.submit()` is absent, and `requestSubmit()` is not.** Submitting a form is defined as
navigating, and navigation is deliberately absent — there is no page to leave. `submit()` is
defined to skip the `submit` event and navigate, so an implementation could only be a silent
no-op or a throw; absent lets feature detection see it.

`requestSubmit([submitter])` is the half that means something without navigation: it fires a
bubbling, cancelable `SubmitEvent` at the form, carrying `submitter`, and does nothing further.
Clicking a submit button does the same, after the click and only if the click was not cancelled.
That is what a single-page application uses — `onsubmit` plus `preventDefault` — and it behaves
exactly as it would in a browser. An application that relied on the navigation gets nothing
instead of the wrong page.

A checkbox or radio clicked without the click being cancelled toggles and fires `input` and
`change`, and a checked radio clears the rest of its group. `form.reset()`, `action` and `method`
stay absent for the same reason `submit()` does: they describe a document navigation.

### What is absent

Constraint validation (`validity`, `checkValidity`, `setCustomValidity`), `labels`, `files`, and
text selection (`select()`, `setSelectionRange`, `selectionStart`/`selectionEnd`) are all absent
rather than stubbed. Each is a surface of its own and each would be a wrong answer if guessed at:
there is no selection model behind an input in this runtime, and no file picker behind one either.

## Storage

`localStorage` and `sessionStorage` exist, hold what is put in them, and **lose it when the
application exits**. There is no profile directory behind an exported application yet, so both are
one process's memory: `sessionStorage` is therefore exactly right, and `localStorage` is a session
store wearing a longer name.

It is implemented anyway because the absence is not survivable and the forgetfulness is. Libraries
read `localStorage` unguarded inside a render — shadcn's theme provider does it in a `useState`
initialiser — so an absent global takes the application down before first paint, while an empty one
degrades to the default theme. What must not happen is that the difference goes unnoticed, so
`doctor` reports every `localStorage.setItem` as `WEB_STORAGE_MEMORY`, on every build, for as long
as this is true. Keep anything that has to outlive the process in a file the application owns.
Real persistence is tracked separately.

`indexedDB` stays absent.

## Device identity

`navigator` answers three questions — `userAgent`, `platform`, and `language`/`languages` — and
nothing else. Those are facts about the machine the application is running on, and they are
answered for the same reason storage is: Svelte 5 reads `navigator.userAgent` while it hydrates,
without guarding it.

Everything else `navigator` normally carries is capability rather than identity — `clipboard`,
`geolocation`, `mediaDevices`, `serviceWorker`, `sendBeacon`, `permissions`, `onLine`,
`userAgentData` — and all of it stays absent, so a feature test selects a fallback instead of
calling something that cannot work. `screen`, `Notification` and `caches` are absent for the same
reason; the native modules cover what an application actually needs there.

The user-agent string names Blitsen (`Blitsen/0.0.0 (Linux x86_64)`) instead of impersonating a
browser. An application that sniffs it deserves a true answer more than it deserves a code path
written for someone else's engine.

## Unreferenced files

Ingest walks the output directory from `index.html` and collects only what it can reach.
Whatever is left over is listed at the end of the build and dropped, because an unreferenced file
is pure export size. Keep some of it with a repeatable glob (`*` stops at `/`, `**` does not):

```sh
npx blitsen build dist --include 'assets/*.wasm' --include 'locales/**'
```

That is also the escape hatch for a file only a runtime-computed URL reaches.

## Where assets live

`--assets embedded` (the default) puts every asset inside the executable and unpacks them into a
private temporary directory at launch — one file to ship, nothing to install. `--assets
side-loaded` writes them to `<outfile>.assets/` beside the executable instead, which is the right
choice when assets must stay patchable after shipping or are large enough that carrying them in
the binary is wasteful. Each asset is content-hashed with SHA-256 either way, and repeating a
build from the same input directory, output path and working directory produces a byte-identical
executable.

## Diagnostic severity

Severity answers one question: **does the page survive?** It is not a measure of how far outside
the profile something is. An ignored paint property, a refused web font, an absent API a library
feature-detects — the page is still there, slightly plainer or on its fallback path. Those are
warnings, reported on every build, and they do not block one.

An error is reserved for the few constructs a page cannot come back from, and the scanner has to
be able to see that the construct is unconditional — a guarded one is not one of these:

| Error | Why the page does not come back from it |
| --- | --- |
| `WEB_FETCH` | A literal server-root URL at a `fetch` call site is not a capability test, so nothing selects a fallback. The data never arrives, and what renders from it never renders. |
| `HTML_CANVAS` | `<canvas>` is in the document the export ships, and the renderer paints nothing inside it. Unlike an image or a font, the element has no degraded appearance to fall back to. |

`ASSET_REMOTE_SCRIPT` used to be the third, on the reading that the loader refusing one remote
`src` left the document running no script at all — which is what stopped wordle-plus loading. The
loader now skips that one script, names it on stderr and runs every other script on the page, so
the reason for the severity is gone and grading it an error only blocks a build that works. What
keeps an exported application from phoning home is the runtime refusing to fetch the script, not
the severity of the rule that noticed it.

**Everything the scanner finds by naming an absent API is a warning**, including `WEB_XHR`,
`WEB_COOKIE`, `WEB_COMPONENTS`, `WEB_CANVAS`, `WEB_NAVIGATION`, `WEB_WORKER`, `WEB_GPU`,
`WEB_DIALOG`, `WEB_STYLE` and `WEB_STORAGE`. What takes a page down is an *unguarded* reference to
an absent global; a guarded one selects a fallback and the page carries on. This scan sees
references, not guards, and in real bundles those references are overwhelmingly guarded —
`typeof XMLHttpRequest<"u"`, `typeof ShadowRoot<"u"`, `"serviceWorker" in navigator`, a
`try`/`catch` around `document.cookie`. Unmodified third-party builds are the evidence:
shadcn-admin carried nineteen such findings and renders its entire admin dashboard, 364 elements
in 16 colours; vue3-realworld carried five and renders. Refusing those builds was the diagnostic
being confidently wrong, and it pointed users at an override that does not exist.

Detecting the guard was the alternative, and it was rejected rather than deferred: the guard is
arbitrary minified JavaScript and may be several frames away from the reference, so a detector
would work often enough to be trusted and then go quiet on the unguarded reference that does kill
a page. Trading a false error for a false silence is a bad trade. The finding is still reported —
every one of them, on every build — at the severity a static reference is actually worth. If your
application uses one of these APIs on a path that runs, the warning is the notice that it will
fail, and the render is what proves it either way.

## Capability tiers

**An unimplemented API is absent — the property does not exist — so feature detection works.**
Never a stub that resolves to nothing, and never a silent no-op. That includes the ones the
Phase 1 Bun host supplies itself: they are deleted while the runtime installs, because an API
that works today and vanishes at the Phase 2 engine swap is worse than one that was never there.

The tables below are **generated from the runtime source**. The surface is installed by
`crates/blitsen-node/src/dom_bridge.rs`, and `packages/blitsen/src/api-manifest.mjs` reads that
file: which globals it defines, what each class declares, and which globals it deletes. `blitsen
doctor` reports from the same manifest, and the native harness asserts every absent entry is
genuinely `undefined` in a real runtime — so the diagnostics, this document and the runtime
cannot drift apart. Regenerate with `bun run --cwd packages/blitsen api:sync`.

Blitsen makes no claim either way about the JavaScript host's own utilities — `URL`,
`URLSearchParams`, `TextEncoder`, `crypto`, `structuredClone`, `performance`, `queueMicrotask`,
`DOMException`, `console` — so they are not listed; the Phase 2 engine has to supply them. Renderer capability (`CSS_*`, `HTML_*`) is not generated
either: no JavaScript declaration describes it, and it is evidenced by the S6 spike and the
determinism gate instead.

<!-- generated: api-manifest -->

| Group | Implemented | Absent |
| --- | --- | --- |
| WEB_DOM | `document`, `Document`, `Node`, `Element`, `NodeList`, `DOMTokenList`, `Attr`, `NamedNodeMap`, `CSSStyleDeclaration`, `MutationObserver`, `HTMLElement`, `HTMLIFrameElement`, `SVGElement`, `Text`, `Comment`, `DocumentFragment`, `HTMLLinkElement`, `HTMLTemplateElement`, `HTMLImageElement`, `Image`, `HTMLImageElement.src`, `HTMLImageElement.naturalWidth`, `HTMLImageElement.naturalHeight`, `HTMLImageElement.complete`, `HTMLImageElement.onload`, `HTMLImageElement.onerror`, `Element.querySelector`, `Element.querySelectorAll`, `Element.closest`, `Element.matches`, `Element.cloneNode`, `Element.contains`, `Element.children`, `Element.previousSibling`, `Element.lastChild`, `Element.parentElement`, `Element.dataset`, `Element.nodeValue`, `Element.before`, `Element.after`, `Element.getElementsByTagName`, `Element.outerHTML`, `Element.insertAdjacentHTML`, `Element.scrollIntoView`, `Element.getElementsByClassName`, `Element.firstElementChild`, `Element.lastElementChild`, `Element.nextElementSibling`, `Element.previousElementSibling`, `Element.childElementCount`, `Element.append`, `Element.prepend`, `Element.replaceChildren`, `Element.getAttributeNS`, `Element.setAttributeNS`, `Element.removeAttributeNS`, `Element.hasAttributes`, `Element.getAttributeNames`, `Element.toggleAttribute`, `Element.getClientRects`, `Element.getRootNode`, `Element.normalize`, `Element.attributes`, `Element.insertAdjacentElement`, `Element.innerText`, `Element.compareDocumentPosition`, `Element.offsetParent`, `Element.clientTop`, `Element.clientLeft`, `Element.hidden`, `Element.tabIndex`, `Element.title`, `Document.title`, `Document.dir`, `Document.getElementsByName`, `Document.elementFromPoint`, `Document.elementsFromPoint`, `Document.scrollingElement`, `Document.characterSet`, `Document.documentURI`, `Document.hasFocus`, `Document.adoptNode`, `HTMLLinkElement.relList`, `HTMLTemplateElement.content`, `DOMTokenList.supports`, `Document.createElementNS`, `Document.createComment`, `Document.createDocumentFragment`, `Document.getElementsByTagName`, `Document.getElementsByClassName`, `Document.importNode` | `Element.attachShadow`, `Document.currentScript` |
| WEB_FORM_CONTROLS | `HTMLInputElement`, `HTMLTextAreaElement`, `HTMLSelectElement`, `HTMLOptionElement`, `HTMLButtonElement`, `HTMLFormElement`, `HTMLInputElement.value`, `HTMLInputElement.defaultValue`, `HTMLInputElement.checked`, `HTMLInputElement.defaultChecked`, `HTMLInputElement.type`, `HTMLInputElement.name`, `HTMLInputElement.disabled`, `HTMLInputElement.form`, `HTMLTextAreaElement.value`, `HTMLTextAreaElement.defaultValue`, `HTMLSelectElement.options`, `HTMLSelectElement.selectedIndex`, `HTMLSelectElement.value`, `HTMLSelectElement.length`, `HTMLSelectElement.selectedOptions`, `HTMLSelectElement.multiple`, `HTMLOptionElement.value`, `HTMLOptionElement.text`, `HTMLOptionElement.selected`, `HTMLOptionElement.index`, `HTMLOptionElement.label`, `HTMLOptionElement.defaultSelected`, `HTMLButtonElement.value`, `HTMLButtonElement.type`, `HTMLFormElement.elements`, `HTMLFormElement.requestSubmit` | `HTMLInputElement.files`, `HTMLInputElement.labels`, `HTMLInputElement.validity`, `HTMLInputElement.checkValidity`, `HTMLInputElement.select`, `HTMLInputElement.setSelectionRange`, `HTMLInputElement.selectionStart`, `HTMLInputElement.selectionEnd`, `HTMLSelectElement.add`, `HTMLFormElement.submit`, `HTMLFormElement.reset`, `HTMLFormElement.action`, `HTMLFormElement.method`, `HTMLFormElement.checkValidity` |
| WEB_EVENTS | `EventTarget`, `Event`, `CustomEvent`, `SubmitEvent`, `MouseEvent`, `KeyboardEvent`, `FocusEvent`, `InputEvent`, `PointerEvent`, `WheelEvent`, `addEventListener`, `removeEventListener`, `dispatchEvent` | — |
| WEB_SCROLL | `scrollTo`, `scrollBy`, `scroll`, `scrollX`, `scrollY`, `pageXOffset`, `pageYOffset` | — |
| WEB_SELECTION | — | `getSelection`, `Range` |
| WEB_SCHEDULING | `requestAnimationFrame`, `cancelAnimationFrame`, `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval` | `requestIdleCallback`, `cancelIdleCallback` |
| WEB_NETWORK | `fetch`, `Headers`, `Request`, `Response`, `Blob`, `AbortController`, `AbortSignal` | — |
| WEB_ROUTING | `window`, `location`, `history`, `Location`, `History`, `PopStateEvent`, `HashChangeEvent` | — |
| WEB_VIEWPORT | `BlitsenViewElement`, `BlitsenViewSurface` | — |
| WEB_STORAGE | `Storage`, `localStorage`, `sessionStorage` | `indexedDB` |
| WEB_WORKER | — | `Worker`, `SharedWorker`, `ServiceWorker`, `ServiceWorkerContainer` |
| WEB_MESSAGING | — | `MessageChannel`, `MessagePort`, `BroadcastChannel`, `postMessage` |
| WEB_SOCKET | `WebSocket`, `MessageEvent`, `CloseEvent`, `WebSocket.url`, `WebSocket.readyState`, `WebSocket.protocol`, `WebSocket.extensions`, `WebSocket.bufferedAmount`, `WebSocket.binaryType`, `WebSocket.send`, `WebSocket.close` | `EventSource` |
| WEB_XHR | — | `XMLHttpRequest` |
| WEB_STREAM | — | `ReadableStream`, `WritableStream`, `TransformStream`, `Response.body`, `Response.clone` |
| WEB_FORM | — | `FormData`, `File`, `FileReader` |
| WEB_CANVAS | — | `HTMLCanvasElement`, `CanvasRenderingContext2D`, `OffscreenCanvas`, `ImageData`, `Path2D` |
| WEB_GPU | — | `WebGLRenderingContext`, `WebGL2RenderingContext`, `GPUCanvasContext` |
| WEB_MEDIA | `Audio`, `AudioContext`, `AudioNode`, `AudioParam`, `AudioBuffer`, `AudioBufferSourceNode`, `AudioDestinationNode`, `GainNode`, `StereoPannerNode`, `HTMLAudioElement`, `AudioContext.decodeAudioData`, `AudioContext.createGain`, `AudioContext.createStereoPanner`, `AudioContext.createBufferSource`, `AudioContext.destination`, `AudioContext.currentTime`, `AudioContext.sampleRate`, `AudioContext.resume`, `AudioContext.suspend`, `AudioContext.close` | `webkitAudioContext`, `HTMLMediaElement` |
| WEB_DIALOG | — | `alert`, `confirm`, `prompt`, `print` |
| WEB_NAVIGATION | `stop` | `open`, `close`, `navigation`, `document.write`, `document.writeln`, `document.open`, `document.close`, `location.assign`, `location.replace`, `location.reload`, `location.ancestorOrigins` |
| WEB_COOKIE | — | `document.cookie`, `cookieStore`, `Headers.getSetCookie` |
| WEB_DEVICE | `Navigator`, `navigator`, `navigator.userAgent`, `navigator.platform`, `navigator.language` | `screen`, `Notification`, `caches` |
| WEB_OBSERVER | `ResizeObserver` | `IntersectionObserver`, `PerformanceObserver` |
| WEB_STYLE | `getComputedStyle`, `matchMedia`, `MediaQueryList`, `MediaQueryListEvent`, `CSS`, `CSSStyleSheet`, `StyleSheetList`, `CSSRule`, `CSSRuleList`, `HTMLStyleElement`, `document.styleSheets`, `HTMLStyleElement.sheet`, `HTMLLinkElement.sheet`, `CSSStyleSheet.cssRules`, `CSSStyleSheet.insertRule`, `CSSStyleSheet.deleteRule`, `CSSStyleSheet.ownerNode`, `CSSStyleSheet.href`, `CSSStyleSheet.title`, `CSSRule.cssText`, `CSSRule.parentStyleSheet` | `CSSStyleRule`, `CSSKeyframesRule`, `CSSKeyframeRule`, `CSSMediaRule`, `document.adoptedStyleSheets`, `CSSStyleSheet.disabled`, `CSSStyleSheet.replaceSync`, `CSSStyleSheet.replace`, `CSSRule.style`, `CSSRule.selectorText`, `CSSRule.type` |
| WEB_COMPONENTS | `DOMParser` | `customElements`, `ShadowRoot` |

| Diagnostic | Severity | Reported as |
| --- | --- | --- |
| `WEB_FETCH` | error | fetch resolves this URL against an address with no server behind it. |
| `WEB_STORAGE_MEMORY` | warning | localStorage is in memory only: what it stores is gone when the application exits. |
| `WEB_DOM` | warning | This DOM method is not implemented. |
| `WEB_FORM_CONTROLS` | warning | This form-control API is not implemented. |
| `WEB_SELECTION` | warning | Text selection and ranges are not implemented. |
| `WEB_SCHEDULING` | warning | Idle-callback scheduling is not implemented. |
| `WEB_STORAGE` | warning | IndexedDB is not implemented. |
| `WEB_WORKER` | warning | Web workers are not implemented. |
| `WEB_MESSAGING` | warning | Message channels are not implemented. |
| `WEB_SOCKET` | warning | Server-sent events are not implemented; WebSocket is. |
| `WEB_XHR` | warning | XMLHttpRequest is not implemented. |
| `WEB_STREAM` | warning | Streaming bodies are not implemented; a response is buffered whole. |
| `WEB_FORM` | warning | Multipart form bodies and file objects are not implemented. |
| `WEB_CANVAS` | warning | Canvas is not in the v0 compatibility profile. |
| `WEB_GPU` | warning | WebGL and WebGPU are not implemented. |
| `WEB_MEDIA` | warning | This media API is not implemented; Web Audio and <audio> are. |
| `WEB_DIALOG` | warning | Modal browser dialogs are not implemented. |
| `WEB_NAVIGATION` | warning | Document navigation is deliberately absent; there is no page to leave. |
| `WEB_COOKIE` | warning | There is no origin and no cookie jar behind an exported application. |
| `WEB_DEVICE` | warning | This device API is not implemented. |
| `WEB_OBSERVER` | warning | This observer is not implemented; only ResizeObserver is. |
| `WEB_STYLE` | warning | This part of CSSOM is not implemented; a sheet's rules are its source text. |
| `WEB_COMPONENTS` | warning | Custom elements and shadow DOM are not implemented; DOMParser is. |
| `CSS_TRANSITION` | warning | A property named by `transition` keeps its pre-stylesheet value (Blitz bug 689). |
| `CSS_FIXED` | warning | Fixed and sticky boxes resolve against the root box, not the viewport (Blitz bug 690). |
| `CSS_EFFECT` | warning | This paint effect is ignored rather than applied. |
| `HTML_CANVAS` | error | <canvas> is not implemented. |
| `HTML_MEDIA` | warning | Video and text tracks are not implemented; <audio> is. |
| `HTML_SVG` | warning | SVG rendering is currently limited and not in the strict profile. |
| `ASSET_REMOTE_SCRIPT` | warning | A remote <script src> is not fetched; it is skipped and the rest of the page runs. |
| `ASSET_REMOTE` | warning | A remote asset is not part of a self-contained export; the request is answered with nothing. |

<!-- /generated -->

The scanner cannot prove visual equivalence or determine that an unsupported reference is dead
code. Treat a zero-error report as the build-time gate and retain visual/interaction acceptance tests
for the application itself. See the earlier [S6 renderer evidence](../spikes/s6/README.md) for why
this boundary exists.

## Native modules

The profile above is what a web application may already assume. The `native:` modules are the
other direction: capability the web has no spelling for, imported under a name that makes the
non-portability obvious at the import site.

```js
import app from "blitsen/app";
import clipboard from "blitsen/clipboard";
```

**`native:` is additive, never a superset** (TECH.md §9). Anything Node already names keeps its
Node name — the command line is `process.argv`, the executable is `process.execPath`, stopping is
`process.exit`, CPU and platform facts are `node:os`, and files are `node:fs`. So there is no
`app.argv` and no `app.quit`: a `native:` member exists only where neither Node nor the web has a
word for the thing.

**Absence is the API.** Outside the runtime — a browser tab, a plain Node script — every access on
these modules throws, because importing them there is a mistake. Inside it, a capability this
build does not implement is genuinely `undefined`, so feature detection works and reads the same
as it does for the web surface:

```js
if (app.requestSingleInstanceLock && !app.requestSingleInstanceLock("My App", relaunchedWith)) {
  process.exit(0);
}
```

The tables are generated from the same runtime source as the tiers above, by the same reader.

### TypeScript

The `blitsen` package carries the definitions, so **editor completion works without the runtime
being loadable in a browser context** — the types resolve from `node_modules`, not from a running
application. Extend the published `tsconfig` fragment:

```json
{ "extends": "blitsen/tsconfig.json", "include": ["src"] }
```

It sets the language level the runtime actually runs, resolves the `blitsen/*` subpaths through
package exports, and adds `blitsen/dom` — which is what declares `<blitsen-view>` and its surface,
including the tag-name map and the JSX namespace, so `document.createElement("blitsen-view")` types
as itself rather than as `HTMLElement`. If you would rather not extend it, reference the DOM types
once anywhere in the project:

```ts
/// <reference types="blitsen/dom" />
```

Each `blitsen/<module>` subpath has **its own declaration file**, so importing `blitsen/app` offers
the app module's members and not the clipboard's. Every member is optional, because a capability the
running version does not implement is `undefined` — which means TypeScript will not let you call one
without the feature detection above, and that is the point.

**The definitions cannot promise an API that does not exist.** They are checked against the
generated manifest in both directions: a declared member the runtime does not install, and an
installed member the definitions do not declare, are each a build failure. `bun run test:types`
typechecks a fixture against the package as it will be published — one file that must compile, one
whose every line must be rejected.

What types do *not* do is describe the absent half of the web surface. `lib.dom.d.ts` will still
offer `IndexedDB` and `HTMLCanvasElement`, because a package cannot remove a global from an ambient
lib. The capability tiers above are the list, and `blitsen doctor` is the check.

<!-- generated: native-modules -->

| Module | Implemented | Absent |
| --- | --- | --- |
| `blitsen/app` | `dataDir`, `cacheDir`, `configDir`, `requestSingleInstanceLock`, `relaunch` | `onQuitRequest`, `onSuspend`, `onResume`, `registerProtocol`, `registerFileAssociation` |
| `blitsen/window` | `setSize`, `setFullscreen`, `isFullscreen`, `setDecorations`, `isDecorated`, `setAlwaysOnTop`, `setCursor`, `setCursorVisible`, `setCursorGrab`, `monitors` | `create`, `setTransparent`, `isAlwaysOnTop` |
| `blitsen/dialog` | `openFile`, `openFiles`, `saveFile`, `openFolder`, `openFolders`, `message` | — |
| `blitsen/clipboard` | `readText`, `readHtml`, `readImage`, `writeText`, `writeHtml`, `writeImage`, `clear` | `readMime`, `writeMime` |

| Absent member | Why |
| --- | --- |
| `app.onQuitRequest` | A close request is a window event, and windows are issue #77's to expose; delivering one from here would mean a second, competing event loop. |
| `app.onSuspend` | Linux has no process-level suspend notification to report. The desktop portals that come closest describe the session, not this application. |
| `app.onResume` | The counterpart of `onSuspend`, absent for the same reason. |
| `app.registerProtocol` | Registering `myapp://` on Linux means installing a `.desktop` entry that names the executable, which is what `blitsen build` already writes. A running process editing that entry would fight its own packaging. The activation itself arrives: the desktop launches the handler with the URL in `argv`, and the single-instance lock hands that to the instance already running. |
| `app.registerFileAssociation` | The same `.desktop` entry, with `MimeType` instead of a scheme. |
| `window.create` | A second window needs the shared-versus-isolated JavaScript context question answered first: whether two windows see one `document` and one module graph or two decides what `create` even returns, and it cannot be settled by implementing it. The window this run already opened is what the rest of this module operates on. |
| `window.setTransparent` | Transparency is chosen when a window is created — winit's own setter does nothing on X11 after that — so honouring it would mean replacing the window, which is `create`. Run `blitsen` against a directory whose window should be transparent and the attribute belongs on that window, not on a call. |
| `window.isAlwaysOnTop` | winit sets the window level and cannot read it back, and the window manager may change it without telling the application. Remembering what was last set would be a second source of truth that quietly goes stale. |
| `clipboard.readMime` | `arboard` reads the flavours above and no others. Arbitrary MIME needs a different mechanism on each platform — X11 selection targets, `wl_data_offer`, `NSPasteboardType`, a registered Windows format — and no part of that is shared. |
| `clipboard.writeMime` | The counterpart of `readMime`, absent for the same reason. |

<!-- /generated -->

### Platform differences

Where a member exists, it means the same thing everywhere. What differs is underneath it.

| Member | What differs |
| --- | --- |
| `app.dataDir`, `app.cacheDir`, `app.configDir` | The application passes its own name, because the runtime does not know one: during development the executable is the host runtime, and a window title is not an identity. The name must be a single path segment. Linux answers with `$XDG_DATA_HOME`/`$XDG_CACHE_HOME`/`$XDG_CONFIG_HOME` and their `~/.local/share`, `~/.cache`, `~/.config` defaults; macOS with `~/Library/Application Support` and `~/Library/Caches`, so data and config are the same directory; Windows with `%APPDATA%` and `%LOCALAPPDATA%`. The directory is returned, never created — making it is `node:fs`. |
| `app.requestSingleInstanceLock` | Unix only, and absent on Windows rather than approximated: the lock is a Unix domain socket in `$XDG_RUNTIME_DIR`, which is both the claim and the channel the second invocation's `argv` and `cwd` arrive on. Windows wants a named mutex and a named pipe, which is a different design rather than this one with the socket swapped. A socket left behind by a process that crashed is detected and taken over. Second invocations are delivered on the frame turn, alongside `fetch` completions, so an application is never re-entered part-way through a frame. |
| `app.relaunch` | Spawns a copy of this process with the same arguments, environment and working directory, and drops the single-instance lock so the successor can take it. It does not stop this process: that is `process.exit`, and only the application knows what it still has to flush. |
| `clipboard.*` | On X11 and Wayland the process that copied is the one that serves the selection, so **what an exported Blitsen application copies disappears when it exits**, unless the desktop runs a clipboard manager that takes a copy. macOS and Windows hand the data to the system and it survives. `writeHtml` also stores the plain text an application that cannot read HTML will paste instead. Images cross as 8-bit RGBA, `{ width, height, data }`, and are carried as PNG on Linux, `CF_DIB` on Windows and an `NSImage` on macOS — a decoded image is not guaranteed to be byte-identical to the one that was copied. A read finds `null` where the clipboard holds nothing in that flavour; a clipboard the session does not offer at all — a headless process — throws instead, because that is an environment refusing rather than an empty clipboard. |
| `window.*` | Operates on the window the run already opened, and is available from the `load` event onwards — document scripts run before the window exists, and a call before then says so rather than doing nothing. `setAlwaysOnTop` reaches the X11 window manager and Windows and macOS; **Wayland has no protocol for stacking a window above others, so the call is accepted and has no effect there**. `setCursorGrab("locked")` is X11-unsupported and `"confined"` is macOS-unsupported; both throw naming the platform rather than silently degrading. `setSize` asks, it does not assert: the size that arrives is whatever the window manager granted, and it is reported by the `resize` event and `innerWidth`/`innerHeight` like any other resize. |
| `dialog.*` | Linux and the BSDs only, through the XDG desktop portal, and absent on macOS and Windows rather than approximated: those platforms require a file dialog on the main thread, which is the thread this design deliberately leaves free to keep painting. Every dialog is modal to the application window and needs one, so these are available from the `load` event onwards like `window.*`. Each returns a **Promise**, and the frame loop keeps turning while the dialog is up — `requestAnimationFrame` still fires, the window still paints, and the answer is delivered on a frame turn alongside `fetch` completions rather than part-way through one. Blocking instead would stop the application repainting for as long as the dialog was open, which X11 and Wayland compositors treat as a hung client. A dismissed file dialog answers `null`: the portal cannot distinguish Cancel from choosing nothing. Paths are real filesystem paths, not `File` objects. Where no portal is running, the desktop's `zenity` is used instead. |

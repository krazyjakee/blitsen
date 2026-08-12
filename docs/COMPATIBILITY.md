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
| Style read-back | `getComputedStyle`, `matchMedia`/`MediaQueryList`, `ResizeObserver` |
| Scheduling | `requestAnimationFrame`, timers and microtasks |
| Networking | `fetch`, `Headers`, `Request`, `Response`, `Blob`, `AbortController` over `http`/`https`, with buffered bodies |
| Routing | In-memory `history` and `location`, `popstate` and `hashchange` |
| CSS | Static block, flex and grid layout; bounded absolute positioning; spacing, borders, backgrounds, colors and system typography |
| Subresources | `<img>` and CSS `background-image` (PNG, JPEG, GIF, WebP), and `@font-face` web fonts (WOFF2, WOFF, TTF, OTF), loaded from local files; SVG images, `<audio>` and `<video>` are not. A subresource the export cannot serve — a remote URL, or a local file that is missing — is answered with an empty body, so the document paints without it rather than waiting on it |

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
| `<script src="https://cdn…">` | Fails the build. The loader refuses the src and then no script on the page runs at all. |

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
(`-->`) is refused rather than silently truncated. `attachShadow` and `scrollIntoView` remain
absent, as does `document.currentScript`: nothing in the bridge is told which script element is
executing.

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
| `ASSET_REMOTE_SCRIPT` | The script loader refuses a remote `src` outright and the document then runs no script at all — not the remote one, not the local ones. It is markup: there is no guard around it and no fallback for it to select. |
| `WEB_FETCH` | A literal server-root URL at a `fetch` call site is not a capability test, so nothing selects a fallback. The data never arrives, and what renders from it never renders. |
| `HTML_CANVAS` | `<canvas>` is in the document the export ships, and the renderer paints nothing inside it. Unlike an image or a font, the element has no degraded appearance to fall back to. |

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
| WEB_DOM | `document`, `Document`, `Node`, `Element`, `NodeList`, `DOMTokenList`, `Attr`, `NamedNodeMap`, `CSSStyleDeclaration`, `MutationObserver`, `HTMLElement`, `HTMLIFrameElement`, `SVGElement`, `Text`, `Comment`, `DocumentFragment`, `HTMLLinkElement`, `HTMLTemplateElement`, `HTMLImageElement`, `Image`, `HTMLImageElement.src`, `HTMLImageElement.naturalWidth`, `HTMLImageElement.naturalHeight`, `HTMLImageElement.complete`, `HTMLImageElement.onload`, `HTMLImageElement.onerror`, `Element.querySelector`, `Element.querySelectorAll`, `Element.closest`, `Element.matches`, `Element.cloneNode`, `Element.contains`, `Element.children`, `Element.previousSibling`, `Element.lastChild`, `Element.parentElement`, `Element.dataset`, `Element.nodeValue`, `Element.before`, `Element.after`, `Element.getElementsByTagName`, `Element.outerHTML`, `Element.insertAdjacentHTML`, `Element.getElementsByClassName`, `Element.firstElementChild`, `Element.lastElementChild`, `Element.nextElementSibling`, `Element.previousElementSibling`, `Element.childElementCount`, `Element.append`, `Element.prepend`, `Element.replaceChildren`, `Element.getAttributeNS`, `Element.setAttributeNS`, `Element.removeAttributeNS`, `Element.hasAttributes`, `Element.getAttributeNames`, `Element.toggleAttribute`, `Element.getClientRects`, `Element.getRootNode`, `Element.normalize`, `Element.attributes`, `HTMLLinkElement.relList`, `HTMLTemplateElement.content`, `DOMTokenList.supports`, `Document.createElementNS`, `Document.createComment`, `Document.createDocumentFragment`, `Document.getElementsByTagName`, `Document.getElementsByClassName`, `Document.importNode` | `Element.attachShadow`, `Element.scrollIntoView`, `Document.currentScript` |
| WEB_FORM_CONTROLS | `HTMLInputElement`, `HTMLTextAreaElement`, `HTMLSelectElement`, `HTMLOptionElement`, `HTMLButtonElement`, `HTMLFormElement`, `HTMLInputElement.value`, `HTMLInputElement.defaultValue`, `HTMLInputElement.checked`, `HTMLInputElement.defaultChecked`, `HTMLInputElement.type`, `HTMLInputElement.name`, `HTMLInputElement.disabled`, `HTMLInputElement.form`, `HTMLTextAreaElement.value`, `HTMLTextAreaElement.defaultValue`, `HTMLSelectElement.options`, `HTMLSelectElement.selectedIndex`, `HTMLSelectElement.value`, `HTMLSelectElement.length`, `HTMLSelectElement.selectedOptions`, `HTMLSelectElement.multiple`, `HTMLOptionElement.value`, `HTMLOptionElement.text`, `HTMLOptionElement.selected`, `HTMLOptionElement.index`, `HTMLOptionElement.label`, `HTMLOptionElement.defaultSelected`, `HTMLButtonElement.value`, `HTMLButtonElement.type`, `HTMLFormElement.elements`, `HTMLFormElement.requestSubmit` | `HTMLInputElement.files`, `HTMLInputElement.labels`, `HTMLInputElement.validity`, `HTMLInputElement.checkValidity`, `HTMLInputElement.select`, `HTMLInputElement.setSelectionRange`, `HTMLInputElement.selectionStart`, `HTMLInputElement.selectionEnd`, `HTMLSelectElement.add`, `HTMLFormElement.submit`, `HTMLFormElement.reset`, `HTMLFormElement.action`, `HTMLFormElement.method`, `HTMLFormElement.checkValidity` |
| WEB_EVENTS | `EventTarget`, `Event`, `CustomEvent`, `SubmitEvent`, `MouseEvent`, `KeyboardEvent`, `addEventListener`, `removeEventListener`, `dispatchEvent` | — |
| WEB_SCHEDULING | `requestAnimationFrame`, `cancelAnimationFrame`, `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval` | `requestIdleCallback`, `cancelIdleCallback` |
| WEB_NETWORK | `fetch`, `Headers`, `Request`, `Response`, `Blob`, `AbortController`, `AbortSignal` | — |
| WEB_ROUTING | `window`, `location`, `history`, `Location`, `History`, `PopStateEvent`, `HashChangeEvent` | — |
| WEB_VIEWPORT | `BlitsenViewElement`, `BlitsenViewSurface` | — |
| WEB_STORAGE | `Storage`, `localStorage`, `sessionStorage` | `indexedDB` |
| WEB_WORKER | — | `Worker`, `SharedWorker`, `ServiceWorker`, `ServiceWorkerContainer` |
| WEB_MESSAGING | — | `MessageChannel`, `MessagePort`, `BroadcastChannel`, `postMessage` |
| WEB_SOCKET | — | `WebSocket`, `EventSource` |
| WEB_XHR | — | `XMLHttpRequest` |
| WEB_STREAM | — | `ReadableStream`, `WritableStream`, `TransformStream`, `Response.body`, `Response.clone` |
| WEB_FORM | — | `FormData`, `File`, `FileReader` |
| WEB_CANVAS | — | `HTMLCanvasElement`, `CanvasRenderingContext2D`, `OffscreenCanvas`, `ImageData`, `Path2D` |
| WEB_GPU | — | `WebGLRenderingContext`, `WebGL2RenderingContext`, `GPUCanvasContext` |
| WEB_MEDIA | — | `Audio`, `AudioContext`, `webkitAudioContext`, `HTMLMediaElement` |
| WEB_DIALOG | — | `alert`, `confirm`, `prompt`, `print` |
| WEB_NAVIGATION | `stop` | `open`, `close`, `navigation`, `document.write`, `document.writeln`, `document.open`, `document.close`, `location.assign`, `location.replace`, `location.reload`, `location.ancestorOrigins` |
| WEB_COOKIE | — | `document.cookie`, `cookieStore`, `Headers.getSetCookie` |
| WEB_DEVICE | `Navigator`, `navigator`, `navigator.userAgent`, `navigator.platform`, `navigator.language` | `screen`, `Notification`, `caches` |
| WEB_OBSERVER | `ResizeObserver` | `IntersectionObserver`, `PerformanceObserver` |
| WEB_STYLE | `getComputedStyle`, `matchMedia`, `MediaQueryList`, `MediaQueryListEvent`, `CSSStyleSheet`, `StyleSheetList`, `CSSRule`, `CSSRuleList`, `HTMLStyleElement`, `document.styleSheets`, `HTMLStyleElement.sheet`, `HTMLLinkElement.sheet`, `CSSStyleSheet.cssRules`, `CSSStyleSheet.insertRule`, `CSSStyleSheet.deleteRule`, `CSSStyleSheet.ownerNode`, `CSSStyleSheet.href`, `CSSStyleSheet.title`, `CSSRule.cssText`, `CSSRule.parentStyleSheet` | `CSSStyleRule`, `CSSKeyframesRule`, `CSSKeyframeRule`, `CSSMediaRule`, `document.adoptedStyleSheets`, `CSSStyleSheet.disabled`, `CSSStyleSheet.replaceSync`, `CSSStyleSheet.replace`, `CSSRule.style`, `CSSRule.selectorText`, `CSSRule.type` |
| WEB_COMPONENTS | — | `customElements`, `ShadowRoot`, `DOMParser` |

| Diagnostic | Severity | Reported as |
| --- | --- | --- |
| `WEB_FETCH` | error | fetch resolves this URL against an address with no server behind it. |
| `WEB_STORAGE_MEMORY` | warning | localStorage is in memory only: what it stores is gone when the application exits. |
| `WEB_DOM` | warning | This DOM method is not implemented. |
| `WEB_FORM_CONTROLS` | warning | This form-control API is not implemented. |
| `WEB_SCHEDULING` | warning | Idle-callback scheduling is not implemented. |
| `WEB_STORAGE` | warning | IndexedDB is not implemented. |
| `WEB_WORKER` | warning | Web workers are not implemented. |
| `WEB_MESSAGING` | warning | Message channels are not implemented. |
| `WEB_SOCKET` | warning | Browser network streams are not implemented. |
| `WEB_XHR` | warning | XMLHttpRequest is not implemented. |
| `WEB_STREAM` | warning | Streaming bodies are not implemented; a response is buffered whole. |
| `WEB_FORM` | warning | Multipart form bodies and file objects are not implemented. |
| `WEB_CANVAS` | warning | Canvas is not in the v0 compatibility profile. |
| `WEB_GPU` | warning | WebGL and WebGPU are not implemented. |
| `WEB_MEDIA` | warning | Audio and the media element constructors are not implemented. |
| `WEB_DIALOG` | warning | Modal browser dialogs are not implemented. |
| `WEB_NAVIGATION` | warning | Document navigation is deliberately absent; there is no page to leave. |
| `WEB_COOKIE` | warning | There is no origin and no cookie jar behind an exported application. |
| `WEB_DEVICE` | warning | This device API is not implemented. |
| `WEB_OBSERVER` | warning | This observer is not implemented; only ResizeObserver is. |
| `WEB_STYLE` | warning | This part of CSSOM is not implemented; a sheet's rules are its source text. |
| `WEB_COMPONENTS` | warning | Custom elements, shadow DOM and DOM parsing are not implemented. |
| `CSS_TRANSITION` | warning | A property named by `transition` keeps its pre-stylesheet value (Blitz bug 689). |
| `CSS_FIXED` | warning | Fixed and sticky boxes resolve against the root box, not the viewport (Blitz bug 690). |
| `CSS_EFFECT` | warning | This paint effect is ignored rather than applied. |
| `HTML_CANVAS` | error | <canvas> is not implemented. |
| `HTML_MEDIA` | warning | Audio and video elements are not implemented. |
| `HTML_SVG` | warning | SVG rendering is currently limited and not in the strict profile. |
| `ASSET_REMOTE_SCRIPT` | error | A remote <script src> stops the document loading; no script on the page runs. |
| `ASSET_REMOTE` | warning | A remote asset is not part of a self-contained export; the request is answered with nothing. |

<!-- /generated -->

The scanner cannot prove visual equivalence or determine that an unsupported reference is dead
code. Treat a zero-error report as the build-time gate and retain visual/interaction acceptance tests
for the application itself. See the earlier [S6 renderer evidence](../spikes/s6/README.md) for why
this boundary exists.

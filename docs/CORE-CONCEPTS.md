# Core concepts

Blitsen is an export target for static web applications. Your framework and bundler produce the
HTML, CSS and JavaScript; Blitsen runs that output and packages it with a native runtime.

## The application directory

Every application starts at `index.html`:

```text
dist/
├── index.html
└── assets/
    ├── app-a1b2.js
    ├── app-c3d4.css
    └── logo.svg
```

Point Blitsen at the output directory, not the repository root:

```sh
npx blitsen dist
```

Blitsen does not transpile `.ts`, `.tsx`, `.jsx`, `.vue` or `.svelte` files. It also does not
resolve bare imports such as `import React from "react"`. A development server may transform
those inputs; a directory or export must contain the already-built result.

## Running and building use the same files

`blitsen dist` and `blitsen build dist` resolve the same entrypoint, assets and module graph. Test
the directory run first, then test the exported artifact. Do not use the browser preview as the
only release check because Blitsen intentionally implements a smaller platform.

With no directory, `blitsen` and `blitsen build` find the nearest `package.json` containing a
`blitsen` key, run its optional build command and use its configured output directory. A directory
argument skips configuration discovery and the configured build command.

## Web APIs are a compatibility boundary

Blitsen supplies HTML/CSS rendering, a DOM, events, modules, timers, networking and selected other
browser APIs. It does not try to be a complete browser.

An unimplemented API is normally absent, so use feature detection:

```js
if ("Worker" in globalThis) {
  // Use the worker path.
} else {
  // Use the fallback.
}
```

Run `blitsen doctor dist` to find statically detectable incompatibilities. The scanner cannot know
which minified branches execute, so warnings require judgment and an actual runtime test. [Web API
support](WEB-APIS.md) summarizes the boundary and links to the exact generated matrix.

Blitsen applications are trusted native software. There is no browser sandbox, same-origin policy
or permission prompt. Do not use Blitsen to render arbitrary content you do not control.

## Modules

The runtime supports classic scripts, module scripts, static imports and dynamic `import()` from
the files in the application:

| Specifier | Behavior |
| --- | --- |
| `./chunk.js` or `../vendor/app.js` | Resolves relative to the importing module |
| `/main.js` | Resolves from the application root |
| `blitsen://app/other.js` | Resolves from Blitsen's internal application origin |
| `react` | Refused; bundle bare package imports first |
| `https://example.com/app.js` | Refused; modules are not fetched from the network |
| A path above the application root | Refused |

Use relative imports and `new URL("./asset.png", import.meta.url)` for output that behaves the same
in development and in an executable.

## Assets and reachability

Build starts at `index.html` and follows references in HTML, CSS and static JavaScript imports.
Reachable files are included; unreferenced files are reported and omitted.

Keep a runtime-loaded file with a repeatable include glob:

```sh
npx blitsen build dist --include 'locales/**' --include 'models/*.bin'
```

Use `*` within one path segment and `**` across directories. Prefer relative URLs in HTML, CSS and
JavaScript. Blitsen handles common server-root HTML/CSS paths from bundlers in its private staging
copy, but it does not rewrite JavaScript strings assembled at runtime.

The default `--assets embedded` stores assets inside the executable. `--assets side-loaded` writes
them to `<output>.assets/` beside it when files need to be replaceable or are too large to embed.
The executable and its asset directory must then travel together.

## Navigation and storage

Blitsen supports application navigation, but an exported application has no HTTP server. Prefer
routes and asset paths that can be resolved within the application. Test direct navigation, back
and forward behavior, and refresh-like reloads in the native runtime.

`localStorage` is synchronous and durable under the platform application-data directory, with no
Blitsen-imposed quota. `sessionStorage` belongs to the current JavaScript realm and is discarded on
reload or exit. The standard runtime does not expose a general filesystem API; arbitrary files and
databases still need a native addon. See
[Persistent application data](RECIPES.md#persistent-application-data).

## Native modules

Capabilities with no browser equivalent are package subpaths such as `blitsen/window` and
`blitsen/os`. Import these from application source before bundling:

```js
import windowApi from "blitsen/window";

addEventListener("load", () => {
  windowApi.setSize?.(1024, 720);
});
```

Members are optional because support varies by version and platform. Always feature-detect them.
See [Native APIs](NATIVE-APIS.md) for the available modules and bundler setup.

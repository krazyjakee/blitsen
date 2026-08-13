# Blitsen

Blitsen is an experimental native runtime for applications built from static
HTML, CSS, and JavaScript output. It combines JavaScriptCore with Blitz's native
HTML/CSS renderer without embedding Chromium or an operating-system WebView.

This package is **pre-alpha**. Directory mode is available for the first runtime
milestone when a compatible native runtime package is installed:

```sh
npx blitsen . --width 800 --height 600 --title "My app"
```

It resolves `index.html`, preflights local entrypoint assets, and opens the result
in a native window. Export with:

```sh
npx blitsen build dist --out MyApp
```

The build names each step as it finishes — `⓪ build`, `① ingest`, `② scan`, `③ collect`,
`④ link`, `⑤ package` — with what it produced, and drops to exit code 1 with the offending file
on stderr when it refuses. The executable is Blitsen's own runtime with your application appended
to it, and it is the only file: the JavaScript engine is linked in rather than shipped beside it.
An export links the older Bun host only when your application carries a `.node` addon, because
Node-API is Bun's ([migration note](https://github.com/krazyjakee/blitsen/blob/main/docs/MIGRATION.md)).

`--target` accepts any of the six platform triples and fetches that target's runtime package on
demand, cached between builds ([#72](https://github.com/krazyjakee/blitsen/issues/72)). What it
cannot do from another platform is sign or notarise.

Point Blitsen at your existing build once, in `package.json`, and `npx blitsen build` needs no
arguments at all:

```json
{ "blitsen": { "build": "vite build", "output": "dist", "name": "My App" } }
```

Blitsen runs `build` from that directory and ingests `output`. It never inspects or configures
your build tool — it runs the command you wrote and consumes the directory it left behind. A
directory argument (`npx blitsen build dist`) skips the wrapping, and every flag overrides the
configured value. `name` becomes the window title and the default output file name. The `blitsen`
key of `package.json` is the only config location; its schema ships as
`blitsen/src/config.schema.json`, and `defineConfig` from the package validates the same shape
in JS:

```js
import { defineConfig } from "blitsen";

// Throws here, with the error the CLI would give, instead of at build time.
const config = defineConfig({ build: "vite build", output: "dist", name: "My App" });
```

Check a bundler's static output against the published v1 profile before export:

```sh
npx blitsen doctor dist
```

The M3b gate consumes Vite's untouched default React output, including `/assets/...` URLs. Blitsen
normalizes those references in its private staging directory, mounts React in the exported
executable, and preserves delegated event state. See the repository's compatibility profile for
the deliberately unsupported browser and renderer features reported by `doctor`.

The build starts at `index.html` and collects only what it can reach through HTML, CSS, and static
module references. Anything left over is reported and dropped; keep it with a repeatable glob, and
choose where the collected assets live:

```sh
npx blitsen build dist --include 'locales/**' --assets side-loaded
```

`--assets embedded` is the default and produces one executable containing the Bun host, native
addon, and application assets. `--assets side-loaded` writes them to `<outfile>.assets/` beside
the executable instead. A profile error from `doctor` fails the build.

Give the export a platform identity, and hand the finished artifact to your own signing setup:

```sh
npx blitsen build dist --out MyApp --icon icon.png --app-version 1.2.3 \
  --sign 'codesign --sign "Developer ID Application: …"'
```

One square PNG becomes the icon container the host wants — a `.desktop` entry and the PNG on
Linux, a `.ico` on Windows, an `.icns` inside a real `MyApp.app` bundle with an `Info.plist` on
macOS (`--bundle-id` sets `CFBundleIdentifier`). A prebuilt `.icns`, `.ico` or `.svg` is used as
given. On Windows the icon and the application manifest ship *beside* the executable: Blitsen does
not embed icon or version-info resources into the PE image, and the build says so rather than
pretending otherwise. `--sign` runs your command with the artifact path as its only argument — the
`.app` bundle on macOS, the executable elsewhere — and a non-zero exit fails the build. Blitsen
never handles certificates.

### Building for another platform

```sh
npx blitsen build dist --target win32-x64   # from Linux
```

`--target` takes any of the six triples. The target's runtime is fetched **on demand** rather than
installed six times over on every machine, and cached between builds — under `XDG_CACHE_HOME`,
`~/Library/Caches` or `%LOCALAPPDATA%`, keyed by version as well as by target, because the runtime
and the CLI are one ABI. `BLITSEN_CACHE_DIR` moves it. The launcher is compiled for that target's
own Bun, so the artifact is a real PE, Mach-O or ELF executable for the platform you asked for —
and a runtime that does not match the target is refused rather than linked into an executable that
would fail at `dlopen` in front of a user.

**What a cross-target build cannot do is sign.** Signing and notarisation need the target
platform's own toolchain and its keychain:

| Step | Cross-building |
| --- | --- |
| macOS `.app` bundle, icon, `Info.plist` | yes — file generation only |
| macOS signing (`codesign`) and notarisation (`notarytool`) | **no** — needs a macOS host, Developer ID and Apple ID |
| Windows `.ico` and manifest beside the executable | yes — file generation only |
| Windows code signing (`signtool`) | **no** — needs a Windows host or a signing service |
| Linux `.desktop` entry and icon | yes — file generation only |

So a cross-built artifact is unsigned by construction. Ship it through a signing step on a host of
that platform; `--sign` is that seam, and it takes the same command you would run there. A
cross-built macOS app that is never signed and notarised is refused by Gatekeeper on any machine
that did not build it.

## The native runtime

This package is thin JavaScript — CLI, config, types. The runtime is one prebuilt binary per
target (`@blitsen/linux-x64`, `@blitsen/darwin-arm64`, and the four others), declared as
`optionalDependencies` carrying `os` and `cpu` fields, so your package manager downloads only the
one matching your machine. Install is a download: no postinstall compile step, no Rust toolchain.

The runtime is pinned to this package's version **exactly**, because the two halves are one ABI
built and tested together. Pin the pair by pinning `blitsen` — its lockfile entry is the pin, and
Blitsen adds no second one that could disagree with it. A runtime that is not this version fails
before your build command runs, naming both versions. Every export records what it linked against,
on the build report and inside the executable:

```
Built /home/me/MyApp (12 assets, 58720256 bytes)
Runtime: @blitsen/linux-x64@0.0.1
```

**No platform package is published yet, and only `linux-x64` is built at all.** Until they ship,
Blitsen resolves the runtime from `BLITSEN_NATIVE_PATH`, or from an addon built inside a checkout
of the repository, and otherwise refuses with the platform whose package it wanted rather than
running against something else.

## Native modules

Import them as ordinary package subpaths:

```js
import dialog from "blitsen/dialog";
import window from "blitsen/window";
```

**`blitsen/*` is the recommended form**, because it is a real npm path that every bundler already
resolves with no configuration. The bare `native:*` spelling reads better but is not resolvable:
measured against default configs, esbuild, Vite, webpack and Bun all fail on it, and Rollup only
warns and externalizes it — which is worse, because it silently produces a bundle whose import
resolves nowhere but inside Blitsen.

Both spellings work. If you prefer `native:*`, mark it external with the optional plugin:

```js
import { blitsenVite } from "blitsen/bundler";        // also blitsenRollup, blitsenEsbuild
export default { plugins: [blitsenVite()] };
```

webpack uses `externals` instead, and needs ESM output, which a Blitsen application has anyway:

```js
import { blitsenWebpackExternals } from "blitsen/bundler";
export default {
  externals: [blitsenWebpackExternals()],
  experiments: { outputModule: true },
  output: { module: true, chunkFormat: "module" },
};
```

A module namespace exposes exactly what the running Blitsen version installed, so a capability
this version does not implement yet is `undefined` rather than a function that throws — feature
detection works:

```js
if (dialog.openFile) { … }
```

Outside the Blitsen runtime — a browser, a plain Node script — every access throws instead, because
that is a mistake rather than a missing capability. `blitsen/app`, `blitsen/window`,
`blitsen/dialog` and `blitsen/clipboard` carry members today, some of them platform-specific —
`dialog.*` is Linux and the BSDs only — which is what makes the feature test above the way to ask.

Exports are architecture proofs and are not yet cleared for redistribution: the automated
third-party notice and library-replaceability gate
([#121](https://github.com/krazyjakee/blitsen/issues/121)) is still outstanding, and `blitsen build`
says so on every build. Follow development and read the feasibility results at
[github.com/krazyjakee/blitsen](https://github.com/krazyjakee/blitsen).

The runtime owns its own event loop: timers, promise jobs and I/O completions are drained at the
frame turn, and rAF, layout, paint and present stay synchronous inside a pump. `blitsen run` uses
Bun as the CLI and the development host; the executable a build produces contains neither.

Repository contributors can run the M1 interactive acceptance app on Linux,
macOS, or Windows with:

```sh
bun run --cwd packages/blitsen example:hello
```

The expected result is a resizable native window with a green panel reading `hi`.

Run the M3 architecture proof with:

```sh
bun run --cwd packages/blitsen example:pong
```

Blitsen is an independent project built on Blitz. It is not an official
DioxusLabs project and is not endorsed by DioxusLabs.

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
in a native window. On the current platform, the Phase 1 architecture-proof exporter is:

```sh
npx blitsen build dist --outfile MyApp
```

Check a bundler's static output against the published v0 profile before export:

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
npx blitsen build dist --outfile MyApp --icon icon.png --app-version 1.2.3 \
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
Phase 1 exports are not yet cleared for redistribution: the automated notice and JSC relinking
gate is still outstanding. Follow development and read the feasibility results at
[github.com/krazyjakee/blitsen](https://github.com/krazyjakee/blitsen).

On Linux, Bun remains the JavaScript event-loop owner. The CLI yields between
non-blocking native window pumps, preserving Bun's timer and promise-microtask
semantics while rAF, layout, paint, and present stay synchronous inside a pump.

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

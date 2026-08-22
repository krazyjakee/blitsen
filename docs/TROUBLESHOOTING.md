# Troubleshooting

Start with the exact command that fails and keep the first Blitsen error. The CLI names the build
stage and exits non-zero instead of continuing with a partial artifact.

## `missing application directory`

Pass a directory containing `index.html`:

```sh
npx blitsen dist
```

Or add a `blitsen` object with an `output` directory to `package.json` and run without a directory.
Blitsen does not guess a `dist` name.

## `missing or unreadable entrypoint`

The directory exists but does not contain a readable `index.html`. Check the build output rather
than the project root:

```sh
npm run build
ls dist/index.html
npx blitsen dist
```

## Source files or bare imports are refused

Messages naming `.tsx`, `.jsx`, `.vue`, `.svelte` or a bare import mean Blitsen received source
instead of browser-ready output. Build first:

```sh
npm run build
npx blitsen dist
```

For development source, point Blitsen at the development server rather than its source directory:

```sh
npx blitsen http://localhost:5173
```

## The development-server window is blank

Verify the URL in a browser or with another HTTP client and check the server terminal. Blitsen does
not start the server for you. Bind it to an address reachable from the process, then retry the exact
HTTP or HTTPS URL.

If the page loads but hot reload does not connect, configure the server's explicit HMR host and
client port. Vite projects use `server.hmr.host` and `server.hmr.clientPort`.

## No window appears for a moment after launch

This is deliberate. A window is created hidden and mapped only after the first complete frame has
been painted, so the application is never seen as an empty or half-drawn rectangle. The wait is
whatever it takes to load the document's critical subresources — stylesheets and web fonts — and
to bring up the GPU surface, which is slowest on a cold start.

A wait long enough to look like a hang usually means a stylesheet or font is still outstanding.
Check the paths in the built output, and remember that a remote subresource is not fetched: it is
answered empty rather than waited on. Against a development-server URL, look at the server terminal
for requests that never complete.

## Doctor reports warnings but exits successfully

Warnings describe behavior that may degrade or may be protected by feature detection. Doctor does
not prove that a warning is harmless. Find the call site, confirm whether it executes, add a
fallback where possible and test the built application in Blitsen.

Doctor errors block export. `--accept-errors` exists for an explicitly reviewed exception, but it
does not implement the missing feature.

## A local asset is missing from the export

The collector starts at `index.html`. If JavaScript computes a filename at runtime, include it:

```sh
npx blitsen build dist --include 'locales/**'
```

Use relative URLs and check the build's omitted-file report. For side-loaded assets, keep the
`<output>.assets/` directory beside the executable.

## An image, stylesheet or script URL works in a browser only

Prefer output-relative URLs and configure the bundler with a relative base. Blitsen rewrites common
server-root paths in HTML and CSS, but cannot safely rewrite strings assembled by JavaScript.
Remote scripts and modules are deliberately not fetched by the runtime.

## `output already exists`

Choose a different `--out` path or use `--force` when replacing the existing artifact is intended:

```sh
npx blitsen build dist --out MyApp --force
```

Packaging may create several related outputs, such as a Linux `.desktop` file, a Windows manifest
or a macOS `.app`; an existing companion can also trigger this refusal.

## The target runtime is missing or mismatched

Reinstall the CLI with its optional dependencies enabled and keep the lockfile intact:

```sh
npm install -D --save-exact blitsen@0.1.0
```

The CLI and native runtime must have exactly the same version. Do not independently update an
`@blitsen/<target>` package.

A cross-target build may need registry/network access to download the target runtime. If the cache
is damaged, remove only the version/target entry reported by the error or set `BLITSEN_CACHE_DIR`
to a new cache directory; do not modify application output to work around it.

## Linux fails to load a shared library

Published Linux runtimes require glibc 2.35 or newer plus ALSA, OpenSSL 3, fontconfig and the active
X11 or Wayland display libraries. Install the missing system package using the distribution's
package manager. Headless containers also need a display environment and are not representative of
a user desktop.

## A native API is `undefined`

Support varies by version and target. Feature-detect the member and grade the intended target:

```sh
npx blitsen doctor dist --target win32-x64
```

Dialogs are Linux-only in this release; the Unix single-instance lock is absent on Windows; app,
window, dialog and clipboard native modules are absent on Android. See [Native APIs](NATIVE-APIS.md).

## A window or dialog method says the window is unavailable

Call window-dependent APIs from the `load` event or later. Document scripts can run before the
native window exists:

```js
addEventListener("load", () => {
  windowApi.setSize?.(1024, 720);
});
```

## Data disappears after restart

`localStorage` and `sessionStorage` are both in-memory in this release. Put durable state in a file
under a directory from `blitsen/app`; the returned directory is not created automatically.

## A cross-built application is blocked by the OS

Cross-built artifacts are unsigned unless a suitable signing service was invoked. Sign on the
target platform (or through a supported external service), then complete notarization or reputation
requirements for that OS. The published 0.1.0 Blitsen runtime itself is unsigned.

## Android build tools are not found

Blitsen does not install Rust, the Android SDK/NDK, `cargo-ndk`, `libclang` or a JDK. Install those
tools and make their normal environment variables/commands visible. The Android crate must also be
available through the checkout or `BLITSEN_ANDROID_CRATE`.

## Get more diagnostic detail

- Run `npx blitsen --version` and record the target OS/architecture.
- Run `npx blitsen doctor dist --json` and keep the complete report.
- Keep the full build output, including the numbered stage where it stopped.
- Reduce the failure to a static `index.html` and its reachable assets if possible.
- Search or open an issue in the [Blitsen repository](https://github.com/krazyjakee/blitsen/issues)
  with the reproduction, expected result and actual output.

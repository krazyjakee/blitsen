# Platform support

Blitsen publishes desktop runtimes for Linux, macOS and Windows on x64 and arm64. Android APK
output exists as a source-checkout workflow and is not installed as a seventh desktop runtime.

Blitsen is pre-alpha. Platform support means that a runtime is produced, not that every application
or operating-system integration behaves identically. Test the exported artifact on each target.

## Desktop targets

| Target | Runtime package | Notes |
| --- | --- | --- |
| `linux-x64` | `@blitsen/linux-x64` | Linux x64 |
| `linux-arm64` | `@blitsen/linux-arm64` | Linux arm64 |
| `darwin-x64` | `@blitsen/darwin-x64` | Intel macOS |
| `darwin-arm64` | `@blitsen/darwin-arm64` | Apple silicon macOS |
| `win32-x64` | `@blitsen/win32-x64` | Windows x64 |
| `win32-arm64` | `@blitsen/win32-arm64` | Windows arm64 |

Only the package matching the install machine is downloaded. A cross-target build fetches the
requested runtime separately and stores it in the platform cache.

## Linux requirements

Published Linux runtimes are built on Ubuntu 22.04 and require glibc 2.35 or newer. The machine
must also provide ALSA, OpenSSL 3, fontconfig and the display libraries needed by its active X11 or
Wayland session.

Minimal containers and headless Linux systems commonly omit these libraries. Blitsen is a windowed
runtime, so a successful install does not imply that such an environment can open an application.

Linux is currently the only desktop platform with `blitsen/dialog`. `setAlwaysOnTop` has no effect
on Wayland because that protocol does not expose the operation. Cursor grab modes also vary; the
runtime throws when a requested mode is unavailable.

## macOS requirements

Blitsen publishes Intel and Apple silicon runtimes. The published 0.1.0 artifacts are unsigned, and
an application you export is unsigned unless your build runs an appropriate signing command.

Distribute a macOS application only after signing its `.app` bundle and completing notarization on
macOS. The current `blitsen/dialog` module is absent on macOS.

## Windows requirements

Published runtimes support Windows 10 or newer, and x64 also supports Server 2016 or newer. The
Microsoft C runtime is statically linked, so users do not need a separate Visual C++ Redistributable.

Windows packaging writes the application manifest and optional `.ico` beside the executable rather
than embedding them in the PE file. Keep those files with the executable. The current
`blitsen/dialog` module and Unix single-instance lock are absent on Windows.

## Android

Android output is an APK built from a Blitsen source checkout. It supports `arm64-v8a` and `x86_64`
by default; `armeabi-v7a` can be requested but has not been run by this project. Android does not
support Blitsen's app, clipboard, dialog or window native modules in this release.

The output is an APK for direct installation, not an Android App Bundle. It cannot be used to
create a new Google Play listing that requires AAB upload. See [Build an Android
APK](PACKAGING.md#build-an-android-apk) for prerequisites and signing.

## Important runtime limitations

- `<canvas>` 2D, WebGL, WebGPU and WebRTC are not implemented.
- There is no platform accessibility tree, so screen readers cannot access the application.
- Text input lacks complete IME/composition, clipboard editing, undo/redo, `contenteditable` and
  complex-script support.
- Cross-platform font fallback is incomplete. Verify typography on every target.
- `Intl` and WebAssembly are absent from the standard shipped JavaScript engine.
- `localStorage` and `sessionStorage` are in-memory and reset when the process exits.
- The runtime is not a browser sandbox and must not run untrusted third-party pages.

This list calls out release-level constraints, not every missing web API. Use `blitsen doctor` and
[Web API support](WEB-APIS.md) for the complete boundary.

## Unsigned artifacts

The published 0.1.0 runtimes are unsigned, and Blitsen does not own or manage your certificates.
Use `--sign` to connect your build to a signing command, then follow the target platform's normal
distribution and notarization process. A cross-target build can generate packaging files but needs
the target's tools or an external signing service to establish publisher identity.

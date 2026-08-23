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
runtime throws when a requested mode is unavailable. Declarative and runtime tray control—including
nested actions, checkboxes, radio groups, accelerators and action/submenu PNGs—notification
submission/lifecycle events and focused native input snapshots are available on desktop targets.
`os.batteries` reads the machine's own batteries on every desktop target and answers an empty list
where there are none; `os.displays` and `os.idleTime` are absent by decision, the monitors being
`window.monitors()` and idle time having no answer a Wayland client can trust.
Checkable tray icons and hidden menu items are not exposed because the native backends do not agree
on them. Linux and
macOS notifications can be updated and closed through their session ID. The installed Windows
notification library delivers interaction events but rejects update and close because it does not
retain an addressable toast handle (#251). Individual notification-server policies still decide
how a submitted notification is presented.

The standard Web `Notification` facade is installed on Linux and in eligible packaged macOS apps.
It is absent on Windows until the notification library exposes addressable close (#251), absent in
an unbundled macOS development host (#253), and absent on Android until intent activation is wired
through #252. The native `blitsen/notify` module is available on every desktop target and Android,
and exposes its platform limits directly.

## macOS requirements

Blitsen publishes Intel and Apple silicon runtimes. The published artifacts are unsigned, and
an application you export is unsigned unless your build runs an appropriate signing command.

Distribute a macOS application only after signing its `.app` bundle and completing notarization on
macOS. Modern macOS notifications also require the exported `.app` bundle identity and signature;
permission requests from an unbundled development executable reject. The current `blitsen/dialog`
module is absent on macOS.

## Windows requirements

Published runtimes support Windows 10 or newer, and x64 also supports Server 2016 or newer. The
Microsoft C runtime is statically linked, so users do not need a separate Visual C++ Redistributable.

Windows packaging writes the application manifest and optional `.ico` beside the executable rather
than embedding them in the PE file. Keep those files with the executable. The current
`blitsen/dialog` module and Unix single-instance lock are absent on Windows.

## Android

Android output is an APK built from a Blitsen source checkout. It supports `arm64-v8a` and `x86_64`
by default; `armeabi-v7a` can be requested but has not been run by this project. Android supports
the focus-scoped `input.snapshot` member and `blitsen/notify`. Notifications use Android's stable
`blitsen.default` channel; API 33+ requests `POST_NOTIFICATIONS`, while API 26–32 reports permission
as granted. Submission, same-session replacement and close are supported. Tap, action and dismiss
events are not exposed until #252 adds Android intent activation, so action-bearing submissions
reject. The standard Web `Notification` global remains absent for the same reason. Android does not
support Blitsen's app, clipboard, dialog, window or tray native modules in this release.

`blitsen/os` is available, and `os.batteries` is the one member of it that is not: the library
behind that reading has no Android backend, and the platform's own answer is `BatteryManager` over
JNI with its own semantics. The input snapshot reports the touch position and a primary button for
the finger that is down; raw pointer movement and wheel deltas stay zero because Android produces
neither, and keys held by physical code exclude the soft keyboard, whose input arrives as DOM
`keydown`.

The output is an APK for direct installation, not an Android App Bundle. It cannot be used to
create a new Google Play listing that requires AAB upload. See [Build an Android
APK](PACKAGING.md#build-an-android-apk) for prerequisites and signing.

## Important runtime limitations

- WebGL, WebGPU and WebRTC are not implemented. `<canvas>` 2D is, without shadows or
  `ctx.filter`.
- There is no platform accessibility tree, so screen readers cannot access the application.
- Text input lacks complete IME/composition, clipboard editing, undo/redo, `contenteditable` and
  complex-script support.
- Cross-platform font fallback is incomplete. Verify typography on every target.
- WebAssembly is absent from the standard shipped JavaScript engine. `Intl` is not: the formatters
  are the runtime's own, over CLDR and the platform's time-zone database, and are the same on every
  target — the database is the system's on Unix, Android's concatenated `tzdata` there, and bundled
  on Windows.
- `localStorage` and `sessionStorage` are in-memory and reset when the process exits.
- The runtime is not a browser sandbox and must not run untrusted third-party pages.

This list calls out release-level constraints, not every missing web API. Use `blitsen doctor` and
[Web API support](WEB-APIS.md) for the complete boundary.

## Unsigned artifacts

The published runtimes are unsigned, and Blitsen does not own or manage your certificates.
Use `--sign` to connect your build to a signing command, then follow the target platform's normal
distribution and notarization process. A cross-target build can generate packaging files but needs
the target's tools or an external signing service to establish publisher identity.

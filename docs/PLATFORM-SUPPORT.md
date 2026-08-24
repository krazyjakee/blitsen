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
must also provide ALSA, OpenSSL 3, fontconfig, libudev and the display libraries needed by its active X11 or
Wayland session.

Minimal containers and headless Linux systems commonly omit these libraries. Blitsen is a windowed
runtime, so a successful install does not imply that such an environment can open an application.

Linux is currently the only desktop platform with `blitsen/dialog`. `setAlwaysOnTop` has no effect
on Wayland because that protocol does not expose the operation. Cursor grab modes also vary; the
runtime throws when a requested mode is unavailable. Declarative and runtime tray control—including
nested actions, checkboxes, radio groups, accelerators and action/submenu PNGs—notification
submission/lifecycle events and focused native input snapshots are available on desktop targets.
Root-element fullscreen is available on every desktop through winit. Pointer lock is currently
exposed on Windows and macOS only: pinned winit 0.31 reports `Locked` cursor grab as unsupported on
X11, and Blitsen does not claim a Linux API that can fail on a common backend. Fullscreen is
borderless on the monitor containing the window (primary fallback), never an exclusive video-mode
switch. Both modes release on Escape, focus loss, surface loss, or target disconnection. Physical
multi-monitor placement and grab behavior still require acceptance on Windows and macOS, with
fullscreen acceptance separately required on X11 and Wayland.
Gamepad snapshots and connect/disconnect events are available on Linux, macOS and Windows through
the target-gated `gilrs` backend. Controllers are sampled once per application redraw, so an idle
window performs no application-side controller polling and learns about a hot-plug on its next
frame. The backend still owns its platform event worker; the additional 50 ms force-feedback loop
is initialized lazily, on the first nonzero effect rather than for controller-free applications.
Standard dual-rumble is exposed only where the device and driver advertise it. Synthetic tests
cover slot, normalization, event, backend-completion and command semantics; physical hot-plug,
mapping and motor behavior still require representative X11, Wayland, Windows and macOS hardware.
`os.batteries` reads the machine's own batteries on every desktop target and answers an empty list
where there are none; `os.displays` and `os.idleTime` are absent by decision, the monitors being
`window.monitors()` and idle time having no answer a Wayland client can trust.
Checkable tray icons and hidden menu items are not exposed because the native backends do not agree
on them. Desktop notifications can be updated and closed through their session ID on every desktop
target; a Windows toast carries that ID as its own tag, which is what an update replaces and a
close removes from the screen and from notification history. Individual notification-server
policies still decide how a submitted notification is presented.

`blitsen/hid` is available on every desktop target, and on Android over a different backend (see
"Android"). On Linux a hidraw node is owned by udev, so a
packaged application reaches an intended device only once a distribution or installer has added a
rule granting access; `blitsen build` writes a `<name>.hid.rules` template beside the executable and
`blitsen doctor` reports the requirement. Blitsen never installs a rule itself and running the
application as root is not a supported substitute.

## Application menu

`blitsen/menu` is present on macOS and Windows and feature-detectably absent on Linux and Android.

- **macOS** installs the NSApp main menu. The required application, edit and window roles are always
  present: Blitsen supplies a standard submenu for each role the application did not claim.
- **Windows** installs a window menu bar on the application's window. Accelerators work because the
  runtime translates them inside winit's message pump; a menu bar shrinks the window's client area,
  as it does for any Win32 application that sets one after creation. `services`, `showAll`,
  `hideOthers`, `fullscreen` and `bringAllToFront` are macOS commands and are omitted rather than
  shown as items that do nothing.
- **Linux** has none. A Linux menu bar is a widget inside the window, and the only backend the menu
  crate has for one is a `gtk::MenuBar` packed into a `gtk::Window` — Blitsen windows are winit's,
  the renderer owns the whole client area, and there is no GTK main loop to run it. The
  desktop-level alternative is the D-Bus global menu, which only some desktops implement and which
  needs an X11 window id, so it answers nothing on Wayland and would give the same application a
  menu on KDE and none on GNOME. The tray menu is not this under another name: it belongs to a
  status item the application may never show.
- **Android** has no application menu bar. Its equivalents — the app bar's overflow menu and the
  navigation drawer — are views inside the activity's own layout rather than a menu the platform
  owns.

The standard Web `Notification` facade is installed on Linux, on Windows, on any macOS process
carrying a bundle identity—an exported application, or a development run inside `blitsen run
--dev-bundle`—and on any Android package the platform launched, where a body tap has an application
identity to be addressed back to. It is absent in an unbundled macOS development host and in an
Android runtime started against a directory standing in for `assets/`. The native `blitsen/notify`
module is available on every desktop target and Android, and exposes its platform limits directly.

### Notifications

A notification outlives the process that showed it, so activating one belonging to a stopped
application is a launch rather than an event. Blitsen delivers that launch context once, on the
first frame turn, as an `activation` event on `notify.onEvent`, and never replays one it has already
delivered—see [Native APIs](NATIVE-APIS.md#cold-start-activation). What each platform does to
produce it differs, and only the parts named here exist:

- **Linux** — an export with `--bundle-id` uses the launch-capable notification portal. The build
  writes `<id>.desktop` with `DBusActivatable=true` and `<id>.service`; once an installer puts those
  in the standard applications and session-service directories, the runtime owns the same bus name,
  registers its host connection as that portal application ID (again whenever the portal service
  restarts) and exports
  `org.freedesktop.Application.ActivateAction`. Body and named-action targets carry the persisted
  envelope, and the portal starts a stopped service before invoking it. Calls from any D-Bus peer
  other than the current `org.freedesktop.portal.Desktop` owner are refused. A development run has no
  identity and retains the freedesktop live-process backend. The portal has no dismissal/expiry
  callback or timeout field, and image-file icons require a sealed descriptor this API does not
  transport, so a packaged app accepts an installed icon-theme name and receives body/action
  activation and explicit `close`, while presentation lifetime and user-dismissal reporting are the
  desktop's policy.
- **Windows** — an export built with `--bundle-id` registers that AppUserModelID with the
  notification platform at startup, which is what gives it a notifier of its own and a permission
  state to read. Windows starts a stopped desktop application for a toast only through a registered
  COM activator implementing `INotificationActivationCallback`; Blitsen does not implement one, so
  toast activation reaches a running process and not a stopped one.
- **macOS** — an exported `.app` is relaunched by the notification centre, and the response is
  delivered to the `UNUserNotificationCenter` delegate. The delegate Blitsen's notification library
  installs discards a response for a notification the running process did not itself submit, so a
  cold-start response is not surfaced.
- **Android** — body, action and delete `PendingIntent`s target a private receiver in the minimal
  `classes.dex`. It persists the activation before body/actions launch the platform
  `NativeActivity` with a clean Intent; swipe dismissal does not open it. The exported launcher
  never reads activation extras, so another application cannot forge an event by explicitly
  starting it. The inbox reaches the current session on its next frame or a later launch, and
  nonces deduplicate repeated delivery.

Where a platform, distribution or installer uses a command-line envelope, the entry point is
`--notification-activation <envelope>` on the application's own command line; both hosts read it,
and a launch without one is an ordinary launch. Linux portal actions carry the same envelope as the
target of `ActivateAction` instead.

## macOS requirements

Blitsen publishes Intel and Apple silicon runtimes. The published artifacts are unsigned, and
an application you export is unsigned unless your build runs an appropriate signing command.

Distribute a macOS application only after signing its `.app` bundle and completing notarization on
macOS. Modern macOS notifications also require a signed `.app` bundle identity, which an export has
and a development run does not: submission and permission from an unbundled executable reject with
a message naming `blitsen run --dev-bundle`, which builds a signed development `.app` around the
interpreter and runs inside it under `com.blitsen.dev.<name>`. No installed application's identifier
is ever borrowed for either. The current `blitsen/dialog` module is absent on macOS.

`blitsen/hid` opens devices with shared IOHID access, so an application never seizes a device from
the rest of the system. A sandboxed application must be signed with `com.apple.security.device.usb`;
`blitsen build` writes a `<name>.app.entitlements` file beside the bundle for the signing command to
pass to `codesign --entitlements`.

## Windows requirements

Published runtimes support Windows 10 or newer, and x64 also supports Server 2016 or newer. The
Microsoft C runtime is statically linked, so users do not need a separate Visual C++ Redistributable.

Windows packaging writes the application manifest and optional `.ico` beside the executable rather
than embedding them in the PE file. Keep those files with the executable. The current
`blitsen/dialog` module and Unix single-instance lock are absent on Windows.

`blitsen/hid` opens HID top-level collections through the Windows HID class driver and needs no
driver installation. Windows reserves some system collections for itself; an open refused that way
rejects with `NotAllowedError`, separately from a device that disappeared.

Windows notification permission is what the notifier reports, so it is `"granted"` or `"denied"`
and never `"default"`; `requestPermission()` reads it without prompting, because Windows gives an
application no prompt to show. Toasts are delivered under an application identity Windows already
knows rather than under `appName`. An export built with `--bundle-id` registers that identity at
startup and files its toasts under it; a development run borrows Windows PowerShell's.
Windows keeps that setting per registered AppUserModelID, so a machine that has registered none—an
image stripped of Start Menu entries, such as a CI runner or a Server Core install—has no notifier
to read: `permission()` and `requestPermission()` reject there, naming the missing identity, rather
than reporting a state nobody chose.

## Android

Android output is an APK built from a Blitsen source checkout. It supports `arm64-v8a` and `x86_64`
by default; `armeabi-v7a` can be requested but has not been run by this project. Android supports
the focus-scoped `input.snapshot` member and `blitsen/notify`. Gamepad globals,
`navigator.getGamepads`, `input.onDeviceChange` and `input.vibrateGamepad` are absent: `gilrs` has no
Android backend, and an always-empty registry would make feature detection lie. Notifications use Android's stable
`blitsen.default` channel; API 33+ requests `POST_NOTIFICATIONS`, while API 26–32 reports permission
as granted. Submission, same-session replacement, close, body taps, action buttons and swipe
dismissal are implemented through the packaged activation bridge. Its manifest, dex build and
persisted handoff are covered deterministically; system-shade interaction and stopped-process
delivery have not yet run on an emulator or device. The standard Web `Notification` global appears
only where that lifecycle contract is present. Android does not support Blitsen's app, clipboard,
dialog, window, tray or menu native modules in this release.

Android rasterises native windows on the CPU and presents the finished buffer through
`ANativeWindow`. This is the shipping default rather than an adapter probe: the API 32/33 CI
emulator's lavapipe adapter cannot satisfy classic Vello's wgpu device request, and physical Mali
and Adreno coverage is not broad enough to make that GPU path a safe default. Source builds retain
`blitsen-android`'s `android-vello-gpu` feature solely to qualify named physical devices; applications cannot
switch renderer at run time.

`blitsen/hid` is present and reaches USB HID devices through `UsbManager`. Enumeration needs no
permission and lists the HID interfaces of every attached USB device; `open()` raises Android's
per-device permission dialog and the promise it returned stays unsettled until that dialog is
answered, resolving on a grant and rejecting with `NotAllowedError` on a dismissal — which can be
asked again. A grant belongs to one device and Android revokes it when that device is unplugged.
Two differences from desktop are worth planning for: `usagePage` and `usage` are `0` in the
enumeration, because a HID report descriptor cannot be read before permission is granted, so filter
by `vendorId` and `productId` there and read the usages after `open()`; and a boot keyboard or
mouse interface is refused before it can be opened, exactly as the desktop collections are.

**This HID path has never been executed.** It type-checks for `aarch64-linux-android` and its logic
is covered by tests on the host, but no report has been exchanged with a real device. The default
renderer no longer asks the CI AVD for the Vulkan capability that blocked application startup, but
an emulator has no USB host controller to attach a HID device to, so the HID acceptance evidence
still needs physical hardware.
`blitsen/os` is available, and `os.batteries` is the one member of it that is not: the library
behind that reading has no Android backend, and the platform's own answer is `BatteryManager` over
JNI with its own semantics. The input snapshot reports the touch position and a primary button for
the finger that is down; raw pointer movement and wheel deltas stay zero because Android produces
neither, and keys held by physical code exclude the soft keyboard, whose input arrives as DOM
composition and `input` events rather than in that snapshot.

The output is an APK for direct installation, not an Android App Bundle. It cannot be used to
create a new Google Play listing that requires AAB upload. See [Build an Android
APK](PACKAGING.md#build-an-android-apk) for prerequisites and signing.

## Important runtime limitations

- WebGL, WebGPU and WebRTC are not implemented. `<canvas>` 2D is, without shadows or
  `ctx.filter`.
- There is deliberately no platform accessibility tree: semantic elements and ARIA attributes do
  not expose roles, names, focus state or live regions to screen readers. Keyboard focus and text
  editing still work through the DOM input path; that does not make the application accessible.
- Editable `<input>` and `<textarea>` controls route winit preedit/commit through composition
  events into a painted Parley composing range, with candidate-window placement on desktop.
  Bounded per-control undo/redo includes selection restoration and committed compositions.
  `contenteditable`, surrounding-text IME deletion, form reset and advanced selection events remain
  absent. Native CJK/RTL input has synthetic coverage only and still needs target-specific human
  verification; static Arabic/RTL and other complex text shaping is a separate tested path.
- Font fallback uses installed system fonts plus application-provided `@font-face` files; no
  universal fallback is bundled. Ship author fonts for stable coverage and metrics. Platform
  emoji, colour fonts and ZWJ sequences still need target-specific verification.
- WebAssembly is absent from the standard shipped JavaScript engine. `Intl` is not: the formatters
  are the runtime's own, over CLDR and the platform's time-zone database, and are the same on every
  target — the database is the system's on Unix, Android's concatenated `tzdata` there, and bundled
  on Windows.
- `localStorage` persists synchronously under the platform application-data directory;
  `sessionStorage` resets with its JavaScript realm.
- The runtime is not a browser sandbox and must not run untrusted third-party pages.

This list calls out release-level constraints, not every missing web API. Use `blitsen doctor` and
[Web API support](WEB-APIS.md) for the complete boundary.

## Unsigned artifacts

The published runtimes are unsigned, and Blitsen does not own or manage your certificates.
Use `--sign` to connect your build to a signing command, then follow the target platform's normal
distribution and notarization process. A cross-target build can generate packaging files but needs
the target's tools or an external signing service to establish publisher identity.

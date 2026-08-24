# Packaging and distribution

`blitsen build` checks static output, collects reachable assets, links the target runtime and adds
platform packaging. The default desktop result embeds the application in one executable.

## Build a desktop artifact

From project configuration:

```sh
blitsen build
```

From an existing output directory:

```sh
blitsen build dist --name "My App" --out MyApp
```

The build stops rather than replacing an existing output. Pass `--force` only when replacement is
intentional.

Run the result on the target operating system and test startup, rendering, input, navigation,
networking and shutdown. A successful build proves that the artifact was created, not that every
application path is compatible.

## Embedded and side-loaded assets

The default embeds reachable application files:

```sh
blitsen build dist --assets embedded
```

Choose side-loaded assets for large or replaceable content:

```sh
blitsen build dist --assets side-loaded
```

This writes `<output>.assets/` beside the executable. Distribute both together. On macOS, when the
build produces a `.app` bundle (pass `--icon`, `--bundle-id` or `--app-version`), Blitsen moves
that directory inside the bundle beside its executable.

Files not reachable from `index.html` are omitted. Keep intentional runtime-loaded files with
repeatable `--include` globs.

## Names, icons and versions

```sh
blitsen build dist \
  --name "My App" \
  --out MyApp \
  --icon assets/icon.png \
  --app-version 1.2.3
```

A PNG is converted into the target platform's normal icon form. You may also provide a native
`.ico`, `.icns` or `.svg` where appropriate.

Packaging differs by target:

| Platform | Output |
| --- | --- |
| Linux | Executable, `.desktop` entry, optional D-Bus `.service` and icon |
| macOS | `.app` bundle with executable, `Info.plist` and optional `.icns` |
| Windows | `.exe`, external application manifest and optional `.ico` |

Set a stable application identity with `--bundle-id com.example.myapp`. `--app-version` is
normalized for the target's metadata format.

## Register the notification entry point

`--bundle-id` is also what a notification activation is addressed to. Without it an artifact has no
identity: two unrelated applications could share one, and notification permission is granted per
identity, so Blitsen never invents one for this purpose — the `com.blitsen.<name>` an `.app` falls
back to for its `Info.plist` deliberately does not become an activation identity.

With it, `blitsen build` records the identity inside the executable and the packaging step writes
what each platform's notification service reads:

| Platform | Registered by the build | Registered at startup |
| --- | --- | --- |
| Linux | `<id>.desktop` with `DBusActivatable=true`, plus `<id>.service` | The runtime owns `<id>` on the session bus, registers that host identity with the portal and exports `org.freedesktop.Application` |
| macOS | The `.app`'s `CFBundleIdentifier` and notification alert style | The response-capturing `UNUserNotificationCenter` delegate |
| Windows | `<name>.exe.notification-register.ps1` with the AppUserModelID and deterministic `LocalServer32` activator class | The same AppUserModelID/COM mapping is refreshed for the executable's current path, and its class factory is registered |
| Android | — | The application ID the package was installed as, read from the Activity |

The Linux files are installer inputs: install the desktop entry under
`$XDG_DATA_HOME/applications` (or `/usr/share/applications`) and the service under
`$XDG_DATA_HOME/dbus-1/services` (or `/usr/share/dbus-1/services`). Leaving them beside the
executable does not register them. The session must provide `xdg-desktop-portal` with the host
application registry and notification interfaces; a packaged `show` rejects with that missing
prerequisite instead of silently submitting a notification that cannot launch the application.

The Windows PowerShell file is an installer input: run it only after the executable is in its final
location. It resolves the executable beside the script rather than embedding the cross-build host's
path. Startup writes the same per-user mapping so a portable build can establish or refresh its
own current path; cross-compilation cannot write the eventual user's registry hive. Android's
application identity is likewise available only at startup from the installed Activity.

Where a platform, distribution or installer uses a command-line launch context, it does so as
`--notification-activation <envelope>` on the application's own command line. Linux portal actions
instead call the exported D-Bus interface with the same envelope. What each platform will actually
start, and what it will not, is [Platform
support](PLATFORM-SUPPORT.md#notifications); what the application receives is [Native
APIs](NATIVE-APIS.md#cold-start-activation).

A development run registers nothing and has no identity. On Linux it retains the freedesktop
live-process backend, so a notification it shows can only be acted on while it is still running;
an activation handed to an identity-less process is refused with a message saying so.

## Sign the artifact

`--sign` runs a command after packaging and passes the finished artifact as its only positional
argument:

```sh
blitsen build dist --sign 'codesign --force --sign "Developer ID Application: Example"'
```

On macOS the argument is the `.app` bundle; elsewhere it is the executable. A non-zero signing
exit code fails the build. Blitsen never reads or stores signing credentials.

Signing is not notarization. Complete the platform's required release process after signing. In
particular, distribute macOS applications only after notarization when Gatekeeper coverage matters.

## Run with a macOS development identity

macOS grants notification permission to an application identity — a bundle identifier and a
signature — and refuses a process that has none. An exported `.app` has one. A development run is an
interpreter executing a script, so it does not, and `blitsen/notify` rejects there rather than
submitting under some other application's name.

```sh
blitsen --dev-bundle
```

This builds a small `.app` around a copy of the interpreter, ad-hoc signs it with `codesign`, and
re-runs the same command line inside it, so the rest of the development loop — the configured build
command, file watching, reload — is unchanged. The bundle is cached beside the runtime cache and
rebuilt when the interpreter, the identifier or the signing command changes.

The identity is the development host's own, `com.blitsen.dev.<name>`, and deliberately not the
`com.blitsen.<name>` an export defaults to: macOS records notification permission per identifier, so
allowing your development host must not read as allowing the application you ship, and revoking one
must not revoke the other. Name your own with `--bundle-id`, and replace the ad-hoc signature with
`--sign` — for a Developer ID identity, or where the interpreter carries entitlements an ad-hoc
re-sign would drop:

```sh
blitsen --dev-bundle --bundle-id com.example.pong.dev \
  --sign 'codesign --force --sign "Developer ID Application: Example"'
```

The flag exists only on macOS and only for `run`; no other desktop platform ties notification
delivery to a bundle identifier, and a build already produces a bundle of its own.

## Build for another desktop target

```sh
blitsen build dist --target win32-x64 --out MyApp.exe
```

Supported triples are:

```text
darwin-arm64  darwin-x64  linux-arm64  linux-x64  win32-arm64  win32-x64
```

Blitsen downloads and caches the exact runtime version for the requested target. Cross-building can
create ELF, Mach-O and PE artifacts and their packaging files, but you still need the target system
for realistic testing and usually for signing/notarization.

## Native addons

Carry a Node-API addon with project configuration or a repeatable flag:

```sh
blitsen build dist --addon native/physics.node
```

An application containing a `.node` addon uses the Bun-based host because the standard runtime has
no Node-API implementation. Building such an export additionally requires `bun` on `PATH`: the Bun
host is linked with `Bun.build`, which only Bun can run, so this is the one export the Node-only
CLI cannot produce alone. This produces a much larger artifact and brings Bun/JavaScriptCore
redistribution requirements that the default notice flow does not automate. Treat addons as an
escape hatch and obtain a licensing review before distribution.

The addon must match the target operating system and architecture. Cross-building does not compile
or translate it.

## Build an Android APK

Android is a separate artifact selected by `--android`, not a desktop target triple:

```sh
blitsen build dist --android --android-abi arm64-v8a --out MyApp.apk
```

The current Android entry crate is not published. Run from a Blitsen source checkout or point the
CLI at one:

```sh
BLITSEN_ANDROID_CRATE=/path/to/blitsen/crates/blitsen-android \
  blitsen build dist --android --out MyApp.apk
```

The build machine needs:

- Rust targets for the requested Android ABIs and `cargo-ndk`
- Android SDK API 33, an NDK and build-tools containing `aapt2`, `d8`, `zipalign` and `apksigner`
- `libclang` for generated QuickJS bindings
- A JDK whose `javac` is on `PATH`, plus `keytool` for the generated debug signing key

Without an ABI option, the APK contains `arm64-v8a` and `x86_64`. Add `--android-debug` for an
unoptimized native build whose manifest is marked debuggable.

Without `--android-keystore`, Blitsen uses the standard debug key. That APK is installable but not
distributable. Supply release credentials without putting passwords on the command line:

```sh
BLITSEN_ANDROID_KEYSTORE_PASSWORD='...' \
  blitsen build dist \
  --android \
  --android-abi arm64-v8a \
  --android-package com.example.myapp \
  --android-keystore /path/to/release.jks \
  --app-version 1.2.3 \
  --out MyApp.apk
```

`BLITSEN_ANDROID_KEY_ALIAS` selects one key in a multi-key store.
`BLITSEN_ANDROID_KEY_PASSWORD` supplies a distinct key password.

Android has no published platform package from which to copy audited notices. Set
`BLITSEN_NOTICES_PATH` to the generated, audited `NOTICES.txt` for the Android crate before
redistribution. Without it the build reports that the APK is not cleared for distribution.

## Third-party notices

Desktop exports embed the notices carried by the resolved runtime package. Inspect the actual
artifact:

```sh
./MyApp --licenses
```

Keep those notices with every distributed copy and follow the source-availability terms named in
them. Read [Licensing](LICENSING.md) for the obligations the default and addon-based exports carry.

## Release checklist

- Build fresh static output and run `blitsen doctor` for every target.
- Review every warning against a real application path.
- Build without `--accept-errors` unless a documented exception is intentional.
- Test the packaged artifact on each target operating system.
- Verify the application name, version, icon and bundle/package identity.
- Sign and, where required, notarize the final packaged artifact.
- Run the artifact with `--licenses` and retain all required notices/source offers.
- Test installation or extraction on a clean machine without a development toolchain.

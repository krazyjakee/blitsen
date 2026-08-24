# CLI reference

The `blitsen` package installs one command with three modes: run, doctor and build.

## Synopsis

```text
blitsen [directory|url] [options]
blitsen build [directory] [options]
blitsen doctor <directory> [--target <triple>] [--json]
```

Use `npx blitsen`, a package-manager equivalent, or a script in `package.json`.

## Run

```sh
npx blitsen dist
npx blitsen http://localhost:5173
```

A directory must contain `index.html`. A URL must use HTTP or HTTPS and points the runtime at an
already-running development server. With no argument, Blitsen uses [project
configuration](CONFIGURATION.md); without configuration it uses the current directory if that
directory contains `index.html`.

Run accepts:

| Option | Default | Meaning |
| --- | --- | --- |
| `--width <pixels>` | `800` | Initial logical window width |
| `--height <pixels>` | `600` | Initial logical window height |
| `--title <text>` | application name or `Blitsen` | Native window title |
| `--dev-bundle` | off | macOS run mode only: wrap the development host in a signed `.app` and relaunch it |
| `--bundle-id <id>` | generated development ID | With `--dev-bundle`, set that `.app`'s `CFBundleIdentifier` |
| `--sign <command>` | ad-hoc signing | With `--dev-bundle`, replace the ad-hoc signature; the `.app` is the command's only argument |

Use a development bundle when exercising a macOS capability whose identity belongs to an
application bundle, notably Notification Center:

```sh
npx blitsen dist --dev-bundle --bundle-id com.example.myapp
```

`--dev-bundle` is rejected by `build`, `doctor`, and non-macOS hosts. In run mode, `--bundle-id`
and `--sign` are rejected unless `--dev-bundle` is also present; ordinary runs have no artifact
for those options to describe.

## Doctor

```sh
npx blitsen doctor dist
npx blitsen doctor dist --target win32-x64
npx blitsen doctor dist --json
```

Doctor scans built static output against the compatibility profile. It exits non-zero for errors;
warnings do not change the exit code. `--target` also grades imports of platform-specific native
modules.

Desktop targets are `darwin-arm64`, `darwin-x64`, `linux-arm64`, `linux-x64`, `win32-arm64` and
`win32-x64`. Doctor additionally accepts `android-arm64` and `android-x64`.

## Build

```sh
npx blitsen build dist --name "My App" --out MyApp
```

Build runs the same compatibility scan, collects reachable assets, links the runtime and packages
the result. Compatibility errors stop the build unless `--accept-errors` is supplied.

### Application and output

| Option | Meaning |
| --- | --- |
| `--name <text>` | Application name, window title and default output name |
| `--title <text>` | Override only the window title |
| `--out <path>` | Output path; defaults to the application name, or without one to the basename of the ingested directory. Windows targets get `.exe` appended |
| `--outfile <path>` | Alias of `--out` |
| `--width <pixels>` | Initial logical width; default `800` |
| `--height <pixels>` | Initial logical height; default `600` |
| `--force` | Replace an existing build output |

### Files and compatibility

| Option | Meaning |
| --- | --- |
| `--include <glob>` | Include an otherwise-unreferenced file; repeatable |
| `--addon <path>` | Carry a `.node` addon; repeatable |
| `--assets embedded` | Store assets in the executable; this is the default |
| `--assets side-loaded` | Write assets to `<output>.assets/` beside the executable |
| `--accept-errors` | Export despite compatibility errors |

Treat `--accept-errors` as an explicit acceptance of broken or degraded behavior, not a normal
release flag.

### Desktop platform and packaging

| Option | Meaning |
| --- | --- |
| `--target <triple>` | Build for another supported desktop target and cache its runtime |
| `--icon <path>` | PNG or a platform-native `.ico`, `.icns` or `.svg` |
| `--bundle-id <id>` | Application identity: the macOS bundle identifier, the Windows AppUserModelID and toast registration, the Linux desktop and D-Bus identity, and the per-application storage identity; also supplies the Android package ID if one is not set. Defaults to `com.blitsen.<title>` |
| `--app-version <version>` | Version recorded in platform metadata; no version is written unless given (Android defaults to `0.1.0`) |
| `--sign <command>` | Run a signing command with the packaged artifact as its only argument |

Cross-building creates the target's files but does not provide its signing or notarization tools.

### Android

Android produces an APK and does not use `--target`; it also rejects `--assets`, `--addon` and —
not yet supported for APKs — `--icon`:

| Option | Meaning |
| --- | --- |
| `--android` | Build an APK instead of a desktop artifact |
| `--android-abi <abi>` | Include `arm64-v8a`, `x86_64` or `armeabi-v7a`; repeatable |
| `--android-package <id>` | Android application ID |
| `--android-keystore <path>` | Sign with a release keystore |
| `--android-debug` | Use an unoptimized, debuggable native build |

Without `--android-abi`, Blitsen includes `arm64-v8a` and `x86_64`. Without a release keystore, it
uses the standard Android debug key. See [Build an Android APK](PACKAGING.md#build-an-android-apk)
for toolchain and credential variables.

## General options

```text
-h, --help       Show CLI help
-v, --version    Show the installed version
```

## Environment variables

Most users do not need these. They are useful for CI, source checkouts and custom toolchains. The
table covers the build-time CLI; variables read by the runtime itself are documented where the
feature is.

| Variable | Purpose |
| --- | --- |
| `BLITSEN_CACHE_DIR` | Override Blitsen's cache directory: fetched cross-target runtimes and the `--dev-bundle` development `.app` |
| `BLITSEN_NATIVE_PATH` | Override the development runtime addon |
| `BLITSEN_RUNTIME_PATH` | Override the executable runtime used for ordinary desktop exports |
| `BLITSEN_ANDROID_CRATE` | Path to the `blitsen-android` crate |
| `BLITSEN_ANDROID_KEYSTORE_PASSWORD` | Android keystore password |
| `BLITSEN_ANDROID_KEY_ALIAS` | Key alias when a keystore contains more than one key |
| `BLITSEN_ANDROID_KEY_PASSWORD` | Key password when it differs from the store password |
| `BLITSEN_NOTICES_PATH` | Audited third-party notices to embed. On desktop it replaces the `NOTICES.txt` beside the linked runtime; on Android it is the only source |
| `BLITSEN_HOST` | Force the export host — `bun` or `blitsen` — instead of letting the exporter choose. A regression escape hatch; see [Migration](MIGRATION.md) |

Runtime overrides are unversioned and must match the requested operating system and architecture;
a `file:` URL is accepted as well as a path. Blitsen validates them before use and reports that
package resolution was bypassed.

The exported executable has a small CLI of its own: `--version`, `--licenses`
([Licensing](LICENSING.md)), `--engine-report` ([JSC](JSC.md)) and internal replay and
notification-activation flags.

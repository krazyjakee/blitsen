# CLI reference

The `blitsen` package installs one command with three modes: run, doctor and build.

## Synopsis

```text
blitsen [directory|url] [options]
blitsen build [directory] [options]
blitsen doctor <directory> [--target <triple>] [--json]
```

After a global install, use `blitsen` directly. A project-local install can instead be invoked by
a script in `package.json` or a package-manager executor such as `npx`.

## Run

```sh
blitsen dist
blitsen http://localhost:5173
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
blitsen dist --dev-bundle --bundle-id com.example.myapp
```

`--dev-bundle` is rejected by `build`, `doctor`, and non-macOS hosts. In run mode, `--bundle-id`
and `--sign` are rejected unless `--dev-bundle` is also present; ordinary runs have no artifact
for those options to describe.

## Doctor

```sh
blitsen doctor dist
blitsen doctor dist --target win32-x64
blitsen doctor dist --json
```

Doctor scans built static output against the compatibility profile. It exits non-zero for errors;
warnings do not change the exit code. `--target` also grades imports of platform-specific native
modules.

Desktop targets are `darwin-arm64`, `darwin-x64`, `linux-arm64`, `linux-x64`, `win32-arm64` and
`win32-x64`. Doctor additionally accepts `android-arm64` and `android-x64`.

## Build

```sh
blitsen build dist --name "My App" --out MyApp
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
| `--sidecar <path>` | Ship a helper executable beside the output, started by name with `blitsen/process`; repeatable. See [Sidecar executables](PACKAGING.md#sidecar-executables) |
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
| `--bundle-id <id>` | Application identity: the macOS bundle identifier, the Windows AppUserModelID and toast registration, the Linux desktop and D-Bus identity, and the per-application storage identity. Defaults to `com.blitsen.<title>` |
| `--app-version <version>` | Version recorded in platform metadata; no version is written unless given |
| `--sign <command>` | Run a signing command with the packaged artifact as its only argument |

Cross-building creates the target's files but does not provide its signing or notarization tools.

### Mobile

Android and iOS are deferred until supported Bun ports exist. `--android`, `--android-*`, and
mobile `--target` values are rejected. Desktop targets all use Bun.

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
| `BLITSEN_NOTICES_PATH` | Audited third-party notices to embed. On desktop it replaces the `NOTICES.txt` beside the linked runtime |
| `BLITSEN_HOST` | Optional legacy environment setting. Only `bun` is accepted; leave it unset. |

Runtime overrides are unversioned and must match the requested operating system and architecture;
a `file:` URL is accepted as well as a path. Blitsen validates them before use and reports that
package resolution was bypassed.

The exported executable has a small CLI of its own: `--version`, `--licenses`
([Licensing](LICENSING.md)), `--engine-report` ([JSC](JSC.md)) and internal replay and
notification-activation flags.

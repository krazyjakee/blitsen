# 0.2.4 — desktop tools, live preferences, and runtime fixes

Blitsen 0.2.4 adds desktop shell and child-process APIs, follows system appearance preferences,
and fixes resource confinement, scaling, and Android notifications. The JavaScript package and
all six native runtimes remain one exactly versioned release.

## Added

- `blitsen/shell` opens web and mail URLs, opens absolute filesystem paths in their associated
  applications, and reveals items in the file manager. External URLs are restricted to `http:`,
  `https:`, and `mailto:`; targets are passed as individual arguments or API parameters.
- `blitsen/process` starts executables with argument arrays, environment and working-directory
  options, streamed output, writable stdin, exit status, and process-tree cancellation. Cleanup
  handles document reloads and descendants that keep output pipes open. Fast children retain
  output for listeners registered when the spawn promise resolves, and Windows children enter
  their job before execution begins.
- `prefers-color-scheme` and `prefers-reduced-motion` follow desktop preferences in stylesheets
  and `matchMedia()` without a reload. Where the system provides no preference, the fallbacks are
  light and no reduced-motion preference.
- Native file, folder, save, and message dialogs are available on macOS and Windows as well as
  Linux. The agent-runner example demonstrates an application for managing local agents.

## Fixed

- Local application resources are confined to their application root, including canonical path
  checks. Development-server resource redirects stay within the configured origin.
- X11 windows respect toolkit scaling, including correctly decoded XSettings byte order.
- Android notification activation loads its receiver through the application's class loader.
  Desktop appearance fallbacks no longer break Android compilation.
- Desktop shell helpers drain stderr while running, avoiding hangs when a handler fills its pipe.
- Build and redistribution checks preserve Cargo's executable-path variables and the licensing
  helper exports used by the package tests.

## Improved

- Native command failures use named `DOMException`s; commands with no result resolve to
  `undefined`. Invalid native API arguments throw at the call, and window setter readbacks and
  unavailable capabilities are documented consistently. Code that expected `null` from HID
  writes or close operations should accept `undefined` instead.
- Settled documents avoid redundant surface attachment and layout resolution. Mutation-observer
  fields are read only when observable, and module loading avoids repeated reads and root lookup.
- Runtime dependencies, examples, and build tooling have been updated.

## Distribution and signing

The npm packages contain a native addon and executable. Release artifacts remain unsigned and
are not notarised; these packaged runtimes are generally not checked by OS gatekeepers during
package-manager installation. Exported applications are different: application authors are
responsible for signing them and, on macOS, notarising them. `blitsen build --sign` is the
integration point. See [`docs/RELEASING.md`](RELEASING.md) and
[`docs/PACKAGING.md`](PACKAGING.md#sign-the-artifact).

# 0.2.6 — sidecar executables

Blitsen 0.2.6 lets an application ship helper executables beside its export and start them
by name. A database service, language server or native core can run as its own process while
the application keeps the standard QuickJS runtime, instead of carrying a Node-API addon and
the larger Bun host. The JavaScript package and all six native runtimes remain one exactly
versioned release.

## Sidecar executables

- `blitsen build --sidecar <path>` (repeatable) or the `sidecars` configuration key copies each
  helper beside the exported executable, keeps its file name and marks it executable. Windows
  targets append `.exe` to a name without an extension. A macOS bundle carries sidecars in
  `Contents/MacOS` before the signing hook runs, so the hook's signature covers them.
- `blitsen/process` starts one with `spawn({ sidecar: "name", args, stdin: "piped" })`. The
  runtime looks beside the running executable, then in the application directory for a
  directory run, and never on `PATH`. A missing sidecar rejects with `NotFoundError` naming
  where it was expected; passing both `command` and `sidecar` is a `TypeError`. A sidecar is
  supervised like any other child: its process tree ends with it and with the application.
- Sidecars work in `blitsen/test` sessions, so a packaged application and its helpers can be
  tested exactly as they ship.

## Build checks

Build refuses, before linking:

- a sidecar that is not an executable for the build target, or is not a file;
- a name that is not a plain file name;
- two sidecars with the same name, or one named like the exported executable, compared
  case-insensitively where the target or build host filesystem is;
- a sidecar that would overwrite a generated artifact such as the side-loaded asset folder or
  a packaging file;
- an existing file at the destination unless `--force` is given.

A sidecar already built into the output directory ships where it is. `--sidecar` and the
`sidecars` configuration key are refused for Android builds, which have no directory beside
the application to ship an executable into. Cross-building does not compile or translate
sidecars; supply one built for the target. See
[Sidecar executables](PACKAGING.md#sidecar-executables) and the child processes section of
[Native APIs](NATIVE-APIS.md).

## Distribution and signing

The npm packages contain a native addon and executable. Release artifacts remain unsigned and
are not notarised; these packaged runtimes are generally not checked by OS gatekeepers during
package-manager installation. Exported applications and their sidecars are different:
application authors are responsible for signing them and, on macOS, notarising them, including
signing each nested executable with the hardened runtime. `blitsen build --sign` is the
integration point. See [Releasing](RELEASING.md) and
[Packaging and distribution](PACKAGING.md#sign-the-artifact).

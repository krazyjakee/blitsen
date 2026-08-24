# 0.2.2 — native APIs, Canvas 2D, and runtime hardening

Blitsen 0.2.2 expands the native desktop surface, makes the existing Canvas 2D API visible in the
rendered frame, and incorporates the reliability and packaging work completed since 0.2.1. The
package and all six native runtimes remain one exactly versioned release.

## Added

- **Canvas 2D painting.** Drawing through a `<canvas>` 2D context now appears in the rendered
  frame, with the runtime's deterministic frame and size gates covering the integrated path.
- **Desktop hardware and system APIs.** The `blitsen` modules now cover HID enumeration, reports
  and hot-plug; gamepad input and haptics; keyboard, pointer and display snapshots; battery state;
  clipboard-change events; and file paths supplied by native drag and drop. Each API reports its
  platform-specific absence explicitly instead of returning an invented value.
- **Native application menus and notifications.** Desktop applications can build native menus and
  use addressable notifications with permission, replacement, timeout and close behavior.
  Notification activation now reaches applications that were not already running on Linux,
  Windows, macOS and Android where the platform permits it.
- **Pointer lock, fullscreen and text editing.** The web surface gains pointer-lock and fullscreen
  behavior, native IME composition, and bounded undo/redo history for text controls.

## Fixed and improved

- `localStorage` is isolated and persisted per application, and multi-window contexts retain their
  documented separation.
- Single-instance handoff is authenticated and acknowledged across Unix sockets and Windows named
  pipes, with shorter macOS socket paths and readiness handling on Windows.
- Standalone packaging bounds bundle memory, cross-checks its Mach-O writers, preserves link-edit
  ordering, and reports diagnostics through source maps.
- Native callbacks, wrapped-value finalizers and runtime locks are hardened against panics,
  re-entrancy and poisoned synchronization state.
- DOM invalidation, event propagation, Canvas command storage, ResizeObserver measurement,
  gamepad polling and window-loop hooks do less repeated allocation and work.
- Release builds stamp one version into both native artifacts and package manifests, verify
  reproducible unsigned outputs on all three desktop operating systems, and enforce the supported
  Node and Rust floors in CI.

## Distribution and signing

The npm packages contain a native addon and executable. Because package managers install those
artifacts inside a dependency tree, unsigned release artifacts are generally not surfaced by
Gatekeeper or SmartScreen during installation. They are nevertheless unsigned and not notarised.

An application produced by `blitsen build` is a directly launched executable and can therefore be
checked by the target operating system. Application authors are responsible for signing and, on
macOS, notarising exported applications; `--sign` is the integration point. See
[`docs/RELEASING.md`](RELEASING.md) and [`docs/PACKAGING.md`](PACKAGING.md#sign-the-artifact).

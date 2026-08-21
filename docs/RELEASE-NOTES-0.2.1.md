# 0.2.1 — native window and system tray configuration

Blitsen 0.2.1 adds declarative desktop presentation controls to the existing `blitsen` package
configuration. Applications can now choose how their first native window is created and add a
system tray icon without application-side native code.

## Added

- **Native window configuration.** `window.type` accepts `normal`, `borderless`, `fullscreen`, or
  `hidden`. The same object controls whether the window is resizable, transparent, or always on
  top.
- **System tray configuration.** A PNG icon, optional tooltip, primary-click behavior, and ordered
  context menu can be declared under `tray`. Menu entries provide built-in `show`, `hide`, `quit`,
  and `separator` actions, with optional labels and enabled state.
- **Close to tray.** `tray.closeToTray` hides the native window when its close control is used.
  Configuration validation requires a `quit` action so the application retains an explicit exit
  path.
- **Portable exports.** Tray icons and settings are carried into both standalone host formats;
  paths are resolved relative to the `package.json` that owns the configuration.

## Validation and compatibility

- Hidden windows require a tray configuration, malformed menu entries are rejected, and configured
  tray icons must be PNG files that exist when an export is assembled.
- The new options are desktop-only. Android builds reject window and tray configuration rather than
  silently ignoring it.
- The JSON Schema, JavaScript validator, TypeScript definitions, CLI, exporter, Bun-hosted runtime,
  and standalone QuickJS runtime share the same configuration shape.
- On Linux, native tray support adds 3.7 MB to the installed standalone runtime and 1.4 MB to its
  compressed size. It uses the StatusNotifierItem D-Bus protocol without requiring GTK or
  AppIndicator development libraries on the user's system.

Tray menus are intentionally declarative in this release: entries invoke the built-in window and
session actions above rather than arbitrary JavaScript callbacks. Published artifacts remain
unsigned and are not notarised. See [`docs/RELEASING.md`](RELEASING.md) for the distribution and
signing model.

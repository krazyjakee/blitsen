# 0.2.3 — reliable window controls and a practical todo example

Blitsen 0.2.3 improves native window interaction, hardens JavaScript callback lifetimes, and turns
the todo example into a small application suitable for real use. The package and all six native
runtimes remain one exactly versioned release.

## Fixed

- Borderless application windows can now be resized from every edge and corner, with native resize
  cursors and platform drag behavior. Maximize, fullscreen, and non-resizable states retain their
  expected boundaries.
- Host callbacks are held by explicit strong engine references. Node-API callbacks therefore remain
  valid across separate native calls and garbage-collection opportunities, matching the existing
  QuickJS lifetime behavior.
- Window-close controls respond reliably to primary-pointer activation, including presses on child
  icon elements, while guarding duplicate close requests and surfacing native failures.
- Mouse wheels and trackpads now translate winit content-motion deltas into the DOM scroll direction.
- Focusing editable controls reconciles against the window's live IME state, avoiding duplicate
  enable requests and preserving cursor-area updates.
- Todo priority controls no longer depend on an unimplemented native select popup. Their accessible
  menus support pointer use, arrow keys, Home, End, Escape, outside-click dismissal, and viewport
  clamping.

## Improved

- The todo example starts with one useful example task instead of generated bulk data. Tasks,
  completion state, and custom priorities persist in `localStorage`; create, edit, delete, clear,
  and undo flows all write through to storage.
- The todo interface has clearer empty states, filtering and search feedback, larger interaction
  targets, keyboard shortcuts, reduced-motion support, focus management, and responsive layout.
- Installation guidance now leads with the global `blitsen` CLI. Projects only need a local exact
  dependency when application code imports `blitsen/*`, or when a team deliberately pins the CLI
  for reproducible CI.

## Distribution and signing

The npm packages contain a native addon and executable. The release artifacts remain unsigned and
are not notarised. Application authors are responsible for signing exported applications and, on
macOS, notarising them; `blitsen build --sign` is the integration point. See
[`docs/RELEASING.md`](RELEASING.md) and [`docs/PACKAGING.md`](PACKAGING.md#sign-the-artifact).

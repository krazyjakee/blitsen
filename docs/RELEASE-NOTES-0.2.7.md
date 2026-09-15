# 0.2.7 — Bun desktop runtime

Blitsen 0.2.7 moves Windows, macOS and Linux to one desktop runtime: Bun runs the
application and its workers, while Blitsen's native addon provides rendering, DOM
bindings and desktop integration. The JavaScript package and all six platform
packages remain one exactly versioned release.

## Runtime and exports

- Desktop runs, packaged exports and `blitsen/test` sessions use Bun. Applications
  can use its `node:*` and `bun:*` modules, including filesystem access and SQLite.
  Blocking database work should run in a worker.
- Bun supplies networking, internationalisation, encoding, URL and body objects,
  structured cloning and workers. Blitsen retains EventSource, application-scoped
  Web Storage and document lifetime management.
- Exports embed a Bun launcher, the native addon and application assets. The
  diagnostic `blitsen-runtime` executable uses the same launcher. Package types
  include the pinned Bun definitions, and exports carry Bun's license inventory.
- Desktop launch handling and document resource cancellation are corrected for
  the Bun runtime, including renderer resource cancellation through `window.stop()`.

## Migration changes

This patch includes compatibility changes from the runtime migration:

- QuickJS is no longer a shipping runtime or export option. Android support is
  withdrawn, and iOS remains unsupported.
- Document modules load through Bun's synchronous `require` path. Move module-level
  `await` into an asynchronous startup function; workers may use top-level `await`.
- Supply absolute URLs to `Request`, choose WebSocket binary types explicitly and
  account for Bun's worker globals. Bare dependencies still need to be bundled or
  included in exported assets.
- The Bun runtime produces larger executables and uses more memory than the former
  QuickJS runtime. See [Bun runtime decision](BUN-MIGRATION.md) for measured examples
  and the complete compatibility boundary.

Desktop sidecars, dialogs, notifications, native input and rendering retain their
existing contracts. Applications remain trusted desktop code with full process access.

## Distribution

Release artifacts remain unsigned and are not notarised. Application authors are
responsible for signing their exports and sidecars and, on macOS, notarising them.
See [Packaging and distribution](PACKAGING.md) and [Licensing](LICENSING.md), including
the Bun/JavaScriptCore source and relinking obligations.

# Module resolution in the shipped binary

**Decision date:** 2026-08-13
**Decision:** a runtime resolver over the application's own files, addressed by an internal
`blitsen://app/` origin, with linking done by the engine's module loader.

This settles [issue #86](https://github.com/krazyjakee/blitsen/issues/86) and TECH.md §17.2.

## The question

Phase 2 drops Bun, and with it Bun's module loader. Two options were on the table:

1. **A pre-bundled single graph.** Require each document to reference one module, evaluate it
   whole, and support nothing else. Simplest possible loader.
2. **A runtime resolver against the embedded files.** Resolve specifiers as they are reached and
   read each module out of the application. Supports dynamic `import()`.

## How much real output depends on being split

Option 1 is only viable if real framework output arrives as one module. It does not.

- **Vite** — the default production build is `rollupOptions`-driven code splitting. Every
  dynamically imported module becomes its own chunk, and shared dependencies are hoisted into
  further chunks joined by static `import`. The repository's own M3b drop-in fixture builds to
  `assets/index-*.js` plus its CSS; add one lazy route and it becomes several files.
- **Routers.** React Router (`lazy`), TanStack Router (`lazyRouteComponent`), Vue Router
  (`() => import(...)`) and SvelteKit all document `import()` as *the* way to code split, and all
  of them are in the audience PRODUCT.md names. Route-level lazy loading is not an edge case
  there; it is the recommended default.
- **The alternative is a downgrade.** Choosing option 1 means telling those users to turn code
  splitting off before Blitsen will run their build. A runtime that only accepts deoptimised
  output is not a target for their toolchain, which is the whole premise (structural constraint 6).

So: **option 2**.

## Structural constraint 6 is not bent

> Blitsen never bundles or transpiles the application. The input is built static output. The
> runtime may load its already-built module graph.

The resolver reads a graph the user's bundler produced. It does not parse the source, rewrite
specifiers, concatenate modules, or transform syntax. `crates/blitsen-host/src/modules.rs` is
about two hundred lines of path arithmetic and a map; nothing in it looks at JavaScript.

## The application origin

A module needs an absolute URL: `import.meta.url` is one by definition, and every relative
specifier is resolved against one. Inside a shipped executable there is no directory to name, so
the application is addressed by an origin of its own.

```
blitsen://app/assets/index-a1b2c3.js
```

TECH.md §17.9 rejected an internal origin, and this does not reverse that. That decision was
about **subresources referenced from HTML and CSS** — `<img src>`, `url()`, `@import` — which are
rewritten to document-relative paths at ingest and never need an origin. Modules are the case the
rewrite cannot cover, because the language hands the URL to the application and the application
does arithmetic on it. The origin exists only where that is true.

The same origin is used for a directory being run and for a bundle inside an executable, so
`blitsen run ./dist` and the exported binary resolve identically. That property is what issue #90
is about, and it would be lost by using `file://` for one and something else for the other.

### Resolution rules

| Specifier | Result |
| --- | --- |
| `./chunk.js`, `../vendor/react.js` | Resolved against the importing module's directory |
| `/main.js` | Resolved against the application root |
| `blitsen://app/other.js` | Taken as it is |
| `react` | Refused, naming it as a bare specifier only a bundler can resolve |
| `https://esm.sh/react`, `//esm.sh/react` | Refused: Blitsen does not fetch modules over the network |
| Anything resolving above the root | Refused |

`?query` and `#fragment` are part of the URL and dropped when the path is resolved, the way a
server drops them before opening a file — a bundler emits both (`?worker`, `?url`).

## Where the graph is linked

Resolution and source are the host's. **Linking** — instantiating records, wiring live bindings,
ordering evaluation, breaking cycles — is the engine's, and no JavaScript engine exposes it to be
reimplemented from outside.

Measured against the system JavaScriptCore this repository builds and tests with:

- A context from `JSGlobalContextCreate` *has* dynamic `import()`, and it rejects with
  `Error: Could not import the module './x.js'` — the default loader cannot fetch.
- `JSLoadAndEvaluateModuleFromSource` is absent. So is any module hook in the GLib API: the only
  evaluation entry points are `jsc_context_evaluate`, `jsc_context_evaluate_in_object` and
  `jsc_context_evaluate_with_source_uri`.

The public C API has no module loader hook at all. This is one of the reasons the acquisition
decision in [`JSC.md`](JSC.md) builds the engine rather than taking one: Blitsen owns a narrow ABI
layer over its pinned WebKit, and that layer is where the hook lives. The contract is recorded in
JSC.md under "Module loader contract".

**Consequence, stated plainly:** built against a JavaScriptCore without that hook — which includes
every system library — an application whose scripts are classic runs normally, and the first
`<script type="module">` fails with a message naming the missing symbol. The M3b React drop-in is
in that second category. Producing the pinned engine artifact is the remaining work for module
support; everything above it is implemented and tested.

## What is implemented

| Piece | Where | Tested by |
| --- | --- | --- |
| Resolution policy | `crates/blitsen-host/src/modules.rs` | `modules::tests` |
| Registry, source reading, reload eviction | same | `modules::tests` |
| Host entry points the loader calls | `ModuleRegistry::install` | `modules::tests` |
| Files from a directory or an appended bundle | `crates/blitsen-host/src/app.rs` | `app::tests` |
| Engine binding and capability check | `crates/blitsen-jsc/src/engine.rs` | `--engine-report` |

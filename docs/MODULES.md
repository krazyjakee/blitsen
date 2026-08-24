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
`blitsen ./dist` and the exported binary resolve identically. That property is what issue #90
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

QuickJS-ng exposes that seam in its stock public API, and rquickjs wraps it with its safe
`Resolver` and `Loader` traits. That pair is exactly what this design needs — the host answers
"what does this specifier mean" and "what is the source", and the engine does the rest. The
runtime links the engine statically, so the hook is a property of the build and cannot be missing
at run time.

**This was the hard part under the previous engine, and the decision that changed it.** The public
JavaScriptCore C API has no module loader hook at all — measured against the system library this
repository used to build against, `JSGlobalContextCreate` gives a context whose dynamic `import()`
rejects with `Error: Could not import the module './x.js'`, `JSLoadAndEvaluateModuleFromSource` is
absent, and the GLib API offers only `jsc_context_evaluate` and its two variants. That is why the
acquisition decision in [`JSC.md`](JSC.md) built the engine rather than taking one: the hook lived
in a patch, so an application whose scripts were classic ran normally while the first
`<script type="module">` failed on a missing symbol. Producing that pinned artifact was the
remaining work for module support. [`spikes/s8`](../spikes/s8/README.md) removed the requirement
instead of meeting it, and the JSC host has since been deleted.

## What is implemented

| Piece | Where | Tested by |
| --- | --- | --- |
| Resolution policy | `crates/blitsen-host/src/modules.rs` | `modules::tests` |
| Registry, source reading, reload eviction | same | `modules::tests` |
| Host entry points the loader calls | `ModuleRegistry::install` | `modules::tests` |
| Files from a directory or an appended bundle | `crates/blitsen-host/src/app/resources.rs` | `app::resources::tests` |
| Engine binding and capability check | `crates/blitsen-quickjs/src/modules.rs` | `--engine-report` |

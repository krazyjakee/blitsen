# Migrating to the Phase 2 runtime

**Nothing changes. Your application gets smaller.**

That is the whole note, and it is the point. Blitsen used to run your application inside Bun and
carry a copy of Bun in every export. It now runs it inside a JavaScript engine Blitsen hosts and
links directly into its own runtime. Bun is still the toolchain; it is no longer in the binary you
ship, and neither is anything else — the export is one file.

## What you do

Nothing. Install the same package, keep the same `"blitsen"` config, run the same commands.

```sh
npm i -D blitsen
npx blitsen build
```

## What changes

Your exported executable is smaller, and it is the only file. Measured on Linux x64 with a bare
application, 144.7 MB became 38.1 MB — 3.46× — with the JavaScript engine linked in rather than
shipped beside it. Your own numbers will differ with your application and platform; run
`blitsen build` and compare.

One kind of application still links the old host: one that carries a `.node` addon. Node-API is
Bun's and the new runtime has none, so an export that could not load your addon would be smaller
and broken. `blitsen build` decides this from what your application carries and says so under step
③, with what the copy of Bun costs.

## What does not change

The npm package, the CLI, its output, the config format, the artifact layout, and what your
application renders. This is checked rather than asserted:
`bun run --cwd packages/blitsen test:hosts` builds one project on both runtimes and fails if the
CLI's output, its config handling, its refusals, the files it produces, or the exported
application's own self-check differ in any way except size. It also replays the committed frame
trace on the new runtime and compares all 120 frames of DOM, layout and pixel digests with what
the old one recorded.

## The two things to know

**`Intl` and `WebAssembly` are not there.** The engine Blitsen hosts does not implement either.
`blitsen doctor` reports both as warnings against your built output, so you find out at build time
rather than in front of a user. The `toLocale*` methods still exist and still return strings — but
they ignore the locale they are given, so `(1234.5).toLocaleString('de-DE')` is `'1234.5'`. A
missed one is wrong output rather than an error, which is why doctor names them.

**Your export is one file.** There is no engine library to keep beside it, no replacement override
to preserve, and no relinking material to ship. That was true of the JavaScriptCore design and is
not true any more; see [`LICENSING.md`](LICENSING.md) for what an export does still have to carry.

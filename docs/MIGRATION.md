# Migrating to the Phase 2 runtime

**Nothing changes. Your application gets smaller.**

That is the whole note, and it is the point. Blitsen used to run your application inside Bun and
carry a copy of Bun in every export. It now runs it inside JavaScriptCore, which Blitsen hosts
itself. Bun is still the toolchain; it is no longer in the binary you ship.

## What you do

Nothing. Install the same package, keep the same `"blitsen"` config, run the same commands.

```sh
npm i -D blitsen
npx blitsen build
```

## What changes

Your exported executable is smaller. Measured on Linux x64 with a bare application, 144.7 MB
became 50.0 MB — 2.89× — plus a replaceable JavaScriptCore library alongside it. Your own numbers
will differ with your application and platform; run `blitsen build` and compare.

## What does not change

The npm package, the CLI, its output, the config format, the artifact layout, and what your
application renders. This is checked rather than asserted:
`bun run --cwd packages/blitsen test:hosts` builds one project on both runtimes and fails if the
CLI's output, its config handling, its refusals, the files it produces, or the exported
application's own self-check differ in any way except size. It also replays the committed frame
trace on the new runtime and compares all 120 frames of DOM, layout and pixel digests with what
the old one recorded.

## The one thing to know

An exported application loads its JavaScript engine from a shared library rather than statically
linking it, which is what lets you distribute a closed-source application without shipping your
own source (see [`LICENSING.md`](LICENSING.md)). Packaging must keep that library alongside the
executable, and must not disable the `BLITSEN_JSC_LIBRARY` override that lets a recipient replace
it. If you use `blitsen build` and its packaging options, this is already handled.

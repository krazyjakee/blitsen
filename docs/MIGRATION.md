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
application, 131.6 MB became 38.1 MB — 3.46× — with the JavaScript engine linked in rather than
shipped beside it. Your own numbers will differ with your application and platform; run
`blitsen build` and compare.

One kind of application still links the old host: one that carries a `.node` addon. Node-API is
Bun's and the new runtime has none, so an export that could not load your addon would be smaller
and broken. `blitsen build` decides this from what your application carries and says so under step
③, with what the copy of Bun costs.

## How long the old host stays selectable

`BLITSEN_HOST=bun` forces the Phase 1 pair, and `BLITSEN_HOST=blitsen` forces the new one. It is an
escape hatch and a measuring instrument, not a supported configuration: neither spelling is a CLI
flag or a config key, because which host an export links is not something a user should have to
decide (structural constraint 7).

It stays for exactly as long as it does something the exporter cannot decide for itself:

- **Until `.node` addons have another answer**, the Bun host is not optional — it is the only one
  with Node-API, and an application that carries an addon links it whether or not the variable is
  set. Nothing here is scheduled for removal while that is true.
- **`BLITSEN_HOST=bun` for an application without an addon** is a regression escape hatch. It is
  supported while both hosts are in the tree and `test:hosts` keeps them both green
  ([#90](https://github.com/krazyjakee/blitsen/issues/90)); it will go in the release after the one
  where nothing has needed it, and the note that goes with that release will say so.
- **The Node surface does not come back either way.** The new runtime implements no `process`,
  `node:fs` or `node:os`, by decision. Code that reaches for them runs on the Bun host today and
  will stop when it goes, so `BLITSEN_HOST=bun` is not a way to depend on them —
  [`COMPATIBILITY.md`](COMPATIBILITY.md#node-compatibility-in-the-shipped-runtime) has what the
  shipped runtime does provide.

## What does not change

The npm package, the CLI, its output, the config format, the artifact layout, and what your
application renders. This is checked rather than asserted:
`bun run --cwd packages/blitsen test:hosts` builds one project on both runtimes and fails if the
CLI's output, its config handling, its refusals, the files it produces, or the exported
application's own self-check differ in any way except size. It also replays the committed frame
trace on the new runtime and compares all 120 frames of DOM digests with what the old one
recorded — layout and pixel digests too, when the rasterizer fingerprint matches the golden's.

## The two things to know

**`WebAssembly` is not there.** The engine Blitsen hosts does not implement it, and `blitsen
doctor` reports it as a warning against your built output, so you find out at build time rather
than in front of a user.

`Intl` *is* there, and it is the runtime's rather than the engine's: number, date, currency,
relative-time, plural, collation and list formatting over CLDR, with named IANA time zones, and
`toLocaleString`/`localeCompare` built on the same formatters. What is absent is the
`formatToParts` family, `Segmenter`, `DisplayNames` and `DurationFormat` — doctor names those.

**Your export is one file.** There is no engine library to keep beside it, no replacement override
to preserve, and no relinking material to ship. All three were required by the JavaScriptCore
design this replaced; see [`LICENSING.md`](LICENSING.md) for what an export does still have to
carry.

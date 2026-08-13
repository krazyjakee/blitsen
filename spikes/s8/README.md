# S8 — QuickJS-ng behind Blitsen's `JsEngine`

Status: **concluded and gated** on Linux x64. The engine seam holds, the size
result is decisive, the performance result is not the one the question expected,
and the golden-frame gate passes — QuickJS-ng reproduces all 120 committed
frames, pixel-identical.

## Question

`JSC.md` chose JavaScriptCore, and `LICENSING.md` requires it to be dynamically
loaded and replaceable because it is LGPL. That decision is what produces the
Phase 2 export's shape: a 37.0 MB executable plus a JavaScript engine library
beside it, and a module loader that needs Bun's patched
`JSLoadAndEvaluateModuleFromSource`. A permissively licensed engine would remove
all three at once — if Blitsen's own `JsEngine` contract can be satisfied by one,
and if P4's 60 fps survives having no JIT.

So: implement the real 33-method trait over QuickJS-ng, then measure.

## Result

**The contract holds.** All eleven checks pass, including the three that were
expected to be awkward: `from_value` re-entrancy, an `ExternalId` finalizer that
runs exactly once, and real weak references.

**Size: 25.3× smaller.** A stripped binary that links the engine statically and
executes JavaScript through it:

| | QuickJS-ng | JavaScriptCore | ratio |
|---|---:|---:|---:|
| engine-only binary, stripped | **1,499,832 B** | 37,980,984 B | 25.3× |
| gzip -9 | 718,828 B | 17,933,416 B | 24.9× |
| library shipped alongside | **none** | 31,959,344 B † | — |

† this machine's `libjavascriptcoregtk-6.0`, which links ICU (36.4 MB) rather
than containing it. The JSC column is from [`spikes/s0`](../s0/README.md), which
measured a static link of Bun's prebuilt archives with ICU included.

**Speed: the ratio depends entirely on what a frame is made of.** QuickJS is
much slower at JavaScript and much faster at crossing into native code:

| | QuickJS-ng | JavaScriptCore | |
|---|---:|---:|---|
| one native callback crossing | **101 ns** | 556 ns | QuickJS 5.5× faster |
| one JavaScript work unit | 30.7 ns | **1.8 ns** | QuickJS 17.1× slower |

Which is why pong — four DOM writes and almost no arithmetic per frame — is
*faster* on the engine without a JIT:

```
pong's own frame(), examples/pong/game.js unmodified — median of 7 runs
  JavaScriptCore   3.5 µs/frame   0.021% of a 16.7 ms budget   (3.4–3.6)
  QuickJS-ng       1.9 µs/frame   0.011% of a 16.7 ms budget   (1.8–2.0)
```

The break-even is **63 JavaScript units per frame**. Below that, the crossing
cost dominates and QuickJS wins; above it, the JIT does. Almost every real
application is above it — but the absolute headroom is what decides P4, and it
is large: **544,057 JavaScript units fit in one 16.7 ms frame on QuickJS**,
against 9,306,483 on JavaScriptCore.

Startup, which P2 cares about:

```
  QuickJS-ng          83 µs to a usable context
  JavaScriptCore     763 µs to a usable context (library already resident)
```

**Bytecode works.** `JS_WriteObject`/`JS_ReadObject` round-trip through the
public API, so an export can ship compiled code and no parser input:
`examples/pong/game.js` is 5,742 B of source and 9,548 B of bytecode — larger,
which is the expected trade for skipping the parse.

## What this does and does not settle

Settled: the `JsEngine` seam is real. A third engine took one file and no change
to `blitsen-host`, `blitsen-dom`, `blitsen-core` or `blitsen-blitz`. The size
claim is measured, not estimated. QuickJS-ng ships `WeakRef`,
`FinalizationRegistry`, `Proxy`, typed arrays, generators and native ES modules.

Not settled, and deliberately so:

- **No Intl, no WebAssembly.** Both absent from QuickJS-ng, and both reachable
  from application code today. That is a `COMPATIBILITY.md` decision, not a
  measurement.
- **This is not the real DOM bridge.** `frame` uses a stub whose property writes
  are native callbacks — the right *shape*, but the real bridge does more per
  crossing. The 5.5× crossing advantage is the number most likely to move.
- **The JavaScriptCore measured here is the system `libjavascriptcoregtk-6.0`,
  not Bun's pinned fork.** A different build with different JIT tuning would
  move the JSC column, though not by the order of magnitude that would change
  the conclusion.
- **Nothing left on this list is a measurement.** Windowed cadence was the last
  one and it has since been taken; see the gate below.

## The gate

`spikes/s8` ended by wiring the engine into `blitsen-runtime` behind
`--features quickjs`, so the committed golden-frame comparison could run against
it. That took one new file — `crates/blitsen-runtime/src/engine.rs`, which holds
the two things that differ between engines: how one is obtained, and how its
module loader is pointed at the registry. Nothing else in the executable, and
nothing at all in `blitsen-host`, changed.

```
blitsen-runtime --replay examples/pong/index.html packages/blitsen/test/replay/pong.trace.json
compared with packages/blitsen/test/replay/pong-linux-x64.golden.json

                fingerprint   dom        layout     pixels
JavaScriptCore  same          120/120    120/120    120/120
QuickJS-ng      same          120/120    120/120    120/120
```

**Pixel-identical for 120 frames, against digests Phase 1 (Bun) recorded.** The
two engines are also byte-identical to each other on all three streams.

Frame cost through the real pipeline — not the stub the benchmarks above use —
from the same replay, over 110 steady frames:

| | QuickJS-ng | JavaScriptCore |
|---|---:|---:|
| mean frame | **875 µs** | 907 µs |
| p95 | **984 µs** | 1,178 µs |
| p99 | **1,192 µs** | 1,605 µs |
| frames over the 16.7 ms budget | **0** | **0** |

And on a real display, vsync-paced, 300 frames after 60 warm-up frames, median
of three runs each:

```
  JavaScriptCore   59.4 fps      (59.4, 60.0, 59.4)
  QuickJS-ng       59.6 fps      (59.6, 59.6, 59.8)
```

Both hold 60 Hz; the acceptance threshold is 58. QuickJS is marginally faster
and materially *steadier*: its tail is tighter because there is no JIT compiling
in the middle of a frame. The per-stage split
shows why the engine barely matters here — `paint` is 757 µs of the frame and is
the same Rust either way, while the JavaScript stages (`animationFrame`,
`animationMicrotasks`) are 60 µs on QuickJS against 84 µs on JavaScriptCore.

What it costs in the executable, and what it saves overall:

| | QuickJS-ng | JavaScriptCore |
|---|---:|---:|
| runtime executable | 38,099,624 B | 36,977,256 B |
| engine library beside it | none | 31,959,344 B |
| **shipped total** | **38,099,624 B** | 68,936,600 B |

The executable grows 1.1 MB by swallowing its engine, and the thing a user
downloads falls by 45%.

## Reproduce

```sh
./run.sh
```

Needs a C toolchain, `libclang` for bindgen, and a JavaScriptCore for the
comparison arms (`libjavascriptcoregtk-6.0` on Ubuntu, or `BLITSEN_JSC_LIBRARY`).
Machine-readable results are in [`results/linux-x64.tsv`](results/linux-x64.tsv).

| binary | what it answers |
|---|---|
| `s8-quickjs` | does the trait hold, and does bytecode round-trip |
| `floor` | the engine-only size number |
| `compare` | synthetic throughput, both engines, same machine |
| `frame` | pong's actual frame cost on both engines |
| `crossover` | crossing cost vs work cost, and where the winner changes |

The gate itself is run from the repository root, not from here:

```sh
cargo build --release -p blitsen-runtime --no-default-features --features quickjs \
  --target-dir target/quickjs
target/quickjs/release/blitsen-runtime --replay \
  "$PWD/examples/pong/index.html" "$PWD/packages/blitsen/test/replay/pong.trace.json"
```

Use absolute paths. A relative entrypoint resolves the stylesheet differently,
which leaves the DOM digests matching and every layout and pixel digest
diverging — a convincing-looking failure that is entirely the invocation.

## Recommendation

The gate that was proposed has been run and it passes: 120 golden frames
pixel-identical, and 59.6 fps windowed against a 58 fps threshold. So the open
question is no longer "does this work" — it is **"is the compatibility trade
acceptable"**, and that is a decision rather than a measurement.

What is being traded away is `Intl` and `WebAssembly`, neither of which
QuickJS-ng has. `COMPATIBILITY.md` promises neither today, but application code
can reach both, so the profile has to say so out loud — and `doctor` already
refuses APIs the profile excludes, so the machinery to say it at build time
exists.

If that clears, this replaces a 37 MB dynamic engine that ships a 32 MB library
beside it, needs a patched entry point for module scripts, and carries an LGPL
relinking obligation — with a 1.5 MB static one that has none of those. The
export shrinks from 68.9 MB shipped to 38.1 MB, and the module-script fallback
that currently sends every Vite application to the 131 MB Bun host disappears,
because this engine loads modules through its stock public API.

If a JIT turns out to be needed for heavier applications, the same seam admits
V8, whose licence solves the shipping problem without the throughput question.
The engine is not the architecture; that is the finding worth keeping.

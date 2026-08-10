# S1 — Bun and winit event-loop ownership

Status: complete on Linux x64. **Choose option 1 for Phase 1: Bun owns the outer
loop and calls winit's non-blocking pump from a repeating JS task.** Option 2 is
not exposed by Bun's supported addon ABI.

## Option 2: winit drives Bun

The probe is a minimal Node-API addon that calls `napi_get_uv_event_loop` and
then attempts one non-blocking `uv_run(..., UV_RUN_NOWAIT)` turn—the only
standard Node-addon route for externally advancing the host loop.

On Bun 1.3.14 (`0d9b296af`) Linux x64, Bun aborts immediately with:

```text
Bun encountered a crash when running a NAPI module that tried to call
the uv_run libuv function.
panic(main thread): unsupported uv function: uv_run
```

This is structural, not a performance failure. On POSIX, Bun uses its uSockets
loop rather than libuv. Its `napi_get_uv_event_loop` implementation returns an
internal Bun `EventLoop*`, while the exported `uv_run` compatibility stub rejects
the call. Bun's internal `us_loop_run_bun_tick`/`tick_without_idle` path is not
exported as Node-API and its VM/event-loop layout is not a stable third-party ABI.
The upstream tracker is [oven-sh/bun#18546](https://github.com/oven-sh/bun/issues/18546).

There is consequently no safe option-2 latency or pacing number to measure: the
prototype cannot advance one turn. Reaching into hidden Rust/uSockets symbols
would tie Blitsen to Bun internals and is rejected.

## Option 1: Bun drives winit

The fallback is a real Rust Node-API addon using winit 0.30.13. A Bun
`setInterval` callback calls `pump_app_events(Some(Duration::ZERO), ...)`; winit
creates an X11 window and handles `UserEvent` and `RedrawRequested` synchronously
inside that pump. A native scheduler thread injects 600 inputs at 16.667 ms
absolute intervals. Their scheduled instants are carried through winit and
measured at the paint callback.

Linux x64 results:

| Work before each pump | Paint mean | Paint stddev | Paint p99 | Input→paint p50 | p95 | p99 | max |
|---|---:|---:|---:|---:|---:|---:|---:|
| idle | 16.073 ms | 0.053 ms | 16.415 ms | 8.190 ms | 15.328 ms | 16.034 ms | 16.148 ms |
| simulated JS/DOM work: 4 ms | 16.070 ms | 0.054 ms | 16.383 ms | 8.204 ms | 15.337 ms | 16.006 ms | 16.092 ms |

The fractional JS interval is effectively scheduled near 16 ms by Bun, yielding
about 62.2 pumps/paint callbacks per second. Production rendering should still
pace presentation to the display rather than treating this timer as vsync. The
important spike result is that the non-blocking pump adds no visible long tail:
all 600 synthetic inputs painted within one 16.667 ms frame in both runs, and a
4 ms per-turn workload did not disturb pacing.

Machine-readable results:

- [`results/fallback-idle-linux-x64.json`](results/fallback-idle-linux-x64.json)
- [`results/fallback-4ms-linux-x64.json`](results/fallback-4ms-linux-x64.json)

This is a host-scheduling measurement, not the final P4 benchmark: it uses a
real window and redraw callbacks but no Blitz layout, wgpu submission, swapchain
present, or physical input device. P4 remains the Pong acceptance measurement.

## Decision and host contract

For Phase 1:

1. Bun stays the outer loop and sole JS-thread owner.
2. A repeating task calls a native `pump_winit` with a zero timeout.
3. Input dispatch, rAF, DOM mutation, layout, paint and present must occur before
   returning from that callback; winit explicitly requires redraw/lifecycle work
   to remain synchronous.
4. Long-running I/O and decode work remains off-thread and only posts results
   back to Bun's loop.
5. The final host replaces the raw 16 ms interval with deadline/display-aware
   pacing and measures missed presents under the Pong workload.

This preserves the single-thread/single-context model. Option 3 (a separate JS
thread and mutation channel) remains the contingency if the full renderer or
later platform validation breaks the one-frame budget; it is not needed by the
Linux result.

Windows and macOS are not measured here. In particular, winit documents macOS
`pump_app_events` caveats around the global `NSApplication`; those targets must
be validated before claiming the Phase 1 host works cross-platform.

## Reproduce

Requirements: Bun, Rust, a C compiler, Node/libuv headers, and an X11 session.

```sh
./spikes/s1/run.sh
```

The script builds and isolates the intentionally crashing option-2 probe, builds
the winit addon, and records idle and 4 ms-workload runs. Override `S1_SAMPLES`
or `S1_PERIOD_MICROS` for shorter diagnostics.

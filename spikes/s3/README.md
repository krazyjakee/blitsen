# S3 — winit + wgpu window from a Bun Node-API addon

Status: complete for the agreed Linux x64 scope. Phase 1 is feasible on Linux.
Windows x64 and macOS arm64 are explicitly deferred.

## Result

A Rust `cdylib` exported through Node-API was loaded synchronously by Bun 1.3.14
on its main thread. From the addon it:

1. constructed a winit 0.30.13 event loop and real 320×180 X11 window;
2. created a wgpu 29 surface, adapter, device, queue and swapchain;
3. submitted a changing color clear and presented it for 120 frames; and
4. kept window/redraw handling synchronous inside the Bun-driven non-blocking
   pump chosen by S1.

Measured result:

| Mode | Window | GPU | Presents | Result |
|---|---|---|---:|---|
| `bun main.js` + addon sidecar | real X11 | RTX 3090 / Vulkan | 120/120 | pass |
| `bun build --compile` executable | real X11 | RTX 3090 / Vulkan | 120/120 | pass |

The compiled executable was launched from a newly created empty working
directory, with no `.node` sidecar available. It still loaded the embedded addon,
opened the window and presented every frame. This confirms Bun's compile step
packages the addon and makes it available through its runtime extraction path.

The release addon was 11,233,584 bytes. The compiled Bun executable containing
the JS entrypoint and addon was 105,814,144 bytes. These are supporting S3
measurements rather than the Phase 2 size budget.

Machine-readable results are in
[`results/linux-x64.json`](results/linux-x64.json).

## Decision

Phase 1's addon architecture survives on Linux: a Bun-loaded Rust addon can own
the window and GPU surface, and the packaged single executable still works. The
implementation must follow S1's ownership rule—Bun stays outermost and calls
winit's zero-timeout pump; rendering happens in `RedrawRequested` before the call
returns.

This does **not** settle the original all-platform risk. In particular, macOS's
`NSApplication` main-thread constraints and winit pump caveats remain unmeasured,
and Windows has not been exercised. Per the user, both are deferred rather than
inferred from Linux. They must be validated before M3 claims the initial P5
platform matrix.

## Reproduce

Requirements: Bun, Rust, an active X11 session and a working wgpu backend.

```sh
./spikes/s3/run.sh
```

Set `S3_FRAMES` to change the default 120-frame run. The script builds the addon,
runs it directly, compiles the JS and addon into a standalone Bun executable,
then runs that executable from an empty temporary directory.

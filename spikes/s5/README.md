# S5 — shared-device wgpu texture composition

This spike proves that an app-owned wgpu texture can be painted as a Blitz
custom widget at its layout position and z-order, using the same device as the
Vello window renderer and the renderer's single surface-present path.

## Run

Linux/X11 with a working wgpu adapter is required:

```sh
spikes/s5/run-linux.sh
```

The harness exits after four presented frames. It first records a renderer-free
Blitz scene with an app-colored widget and asserts that the fill order is DOM
underlay, app widget, DOM overlay. It then opens a real winit/wgpu window and:

1. obtains Vello's type-erased `wgpu_context::DeviceHandle`;
2. creates and clears a double-buffered app texture on that device and queue;
3. registers each texture as an AnyRender resource;
4. lets Blitz size and place the resource from the `<object>` layout box; and
5. submits the combined Blitz scene through one `WindowRenderer::render` call.

## Linux result

Tested on X11 with an NVIDIA GeForce RTX 3090 through Vulkan:

```text
S5_Z_ORDER below=2 app=4 above=5
S5_DEVICE name="NVIDIA GeForce RTX 3090" backend=Vulkan
S5_FRAME frame=1 layout=480x320 resource=ResourceId(0)
S5_SURFACE_FRAME render_call=1 surface_path=single-render-single-present
...
S5_FRAME frame=4 layout=480x320 resource=ResourceId(1)
S5_SURFACE_FRAME render_call=4 surface_path=single-render-single-present
```

The two resource IDs alternate as expected from double buffering. The
layout-derived size is exactly 480×320. Recorded paint-command indices prove
that Blitz inserts the app scene between DOM content with lower and higher
z-order.

`VelloWindowRenderer::render` owns the only surface. For each call it obtains
one current surface texture, renders the combined scene to that texture, and
calls `maybe_blit_and_present` once. The app renderer creates no surface and
never presents; its queue submission only produces the sampled texture.

## Pinned shell regression

The normal `blitz-shell/custom-widget` convenience path at the project pin does
not compile because `complete_resume` passes an immutable renderer reference to
`can_create_surfaces`, which requires a mutable render context. This is tracked
as [DioxusLabs/blitz#679](https://github.com/DioxusLabs/blitz/issues/679).

The spike uses a minimal winit harness around the same pinned Blitz document,
paint, and Vello APIs to isolate that one-line shell error from the composition
mechanism. Once #679 is fixed, Blitsen can use or mirror the normal shell path.

## Decision

The native viewport/custom-widget seam is viable. Use the renderer-provided
device and queue, render app content into registered textures, and let Blitz
place that resource in its paint scene. No second swapchain or surface is
needed. Windows and macOS validation is deferred.

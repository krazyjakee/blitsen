# 0.1.3 — restore the Intel macOS CPU renderer

Blitsen 0.1.3 restores windowing on Intel macOS after 0.1.2 blocked `MacBookPro14,3` based on a
misidentified executable. The reported reset came from the example workspace's local 0.1.0
package, not the globally installed 0.1.1 CPU-rendering package.

## Fixed

- Intel Macs again select Vello's CPU rasterizer and softbuffer presentation automatically.
- The incorrect `MacBookPro14,3` refusal and fail-closed hardware-model check are removed.
- Compatibility and historical release notes distinguish the unsafe 0.1.0 Vello/Metal path from
  the working 0.1.1 CPU path.

## Known limitation

CPU rendering avoids Blitsen's unsafe Vello/Metal compute workload but is substantially slower
than GPU rendering, particularly at HiDPI resolutions and on pages that repaint frequently. The
renderer choice remains visible on stderr as `renderer=vello-cpu window-backend=softbuffer`.

Published macOS artifacts remain unsigned and are not notarised. See
[`docs/RELEASING.md`](RELEASING.md) for the distribution and signing model.

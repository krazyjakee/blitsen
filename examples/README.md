# Examples

These applications exercise Blitsen's supported web surface and native extensions. They are also
useful compatibility checks: each one is deliberately small enough to show which capability makes
it work and, where necessary, which fallback an application still needs.

## Static applications

Each of these directories is already built output. With the published CLI available, open one
from the repository root with `blitsen examples/<name>`. The runner names below are for a
source checkout: `bun run --cwd packages/blitsen <runner>` builds the local addon before opening
the example.

| Example | What it demonstrates | Checkout runner |
| --- | --- | --- |
| [assets](assets/) | Local images, CSS backgrounds, intrinsic image sizing and a bundled `@font-face` | — |
| [audio](audio/) | Web Audio one-shots and overlapping sources, plus `Audio` playback, gain and pan | — |
| [canvas](canvas/) | Canvas 2D paths, gradients, text, images, compositing and pixel readback | `example:canvas` |
| [hardware](hardware/) | Processor, memory, storage and operating-system data from `blitsen/os` | `example:hardware` |
| [hello-dom](hello-dom/) | The smallest HTML/CSS application and a resizable native window | `example:hello` |
| [interactive](interactive/) | Pointer, keyboard, focus, layout and paint updates in one interactive control | `example:interactive` |
| [native-view](native-view/) | An application-drawn `blitsen-view` surface composited with DOM content | `example:native-view` |
| [pong](pong/) | A complete two-player game driven by keyboard input and animation frames | `example:pong` |
| [responsive](responsive/) | Media queries, responsive grid layout, resize events and `ResizeObserver` | — |
| [todo](todo/) | A persistent task list with priorities, search, filters and native window controls | `example:todo` |

The binary files in the assets example are committed so it runs without a generation step. They
were built by [`assets/generate.py`](assets/generate.py), which requires `fonttools` and `brotli`.
Regenerate them from the repository root only when changing the fixture:

```sh
python examples/assets/generate.py examples/assets
```

## Bundled applications

These examples contain framework or editor source rather than runnable output. Install their
locked dependencies and build them before pointing Blitsen at `dist`:

```sh
cd examples/monaco # or examples/reactflow
npm ci
npm run build
blitsen dist
```

The React acceptance example uses its Bun lockfile instead:

```sh
cd examples/vite-react
bun install --frozen-lockfile
bun run build
blitsen dist
```

| Example | What it demonstrates |
| --- | --- |
| [monaco](monaco/) | Monaco Editor, including its TypeScript worker and off-screen text-input focus model |
| [reactflow](reactflow/) | A real React Flow graph with dragging, connections and compatibility fallbacks for current renderer gaps |
| [vite-react](vite-react/) | The Vite/React adoption and conformance application used by the automated test suite |

Examples show the current compatibility boundary rather than promising the whole browser platform.
Consult [Web API support](../docs/WEB-APIS.md) and run `blitsen doctor` against your own built output
before relying on the same APIs in an application.

# Recipes

Practical patterns for common Blitsen application tasks.

## Use Vite, React, Vue or Svelte

Keep the existing build and point Blitsen at its output:

```json
{
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "native:dev": "blitsen http://localhost:5173",
    "native": "blitsen build"
  },
  "blitsen": {
    "build": "vite build",
    "output": "dist",
    "name": "My App"
  }
}
```

For computed asset URLs, configure a relative base where your tool supports it. In Vite:

```js
import { defineConfig } from "vite";

export default defineConfig({
  base: "./",
});
```

Default Vite root asset paths in HTML and CSS are handled by Blitsen, but a relative base also
makes paths assembled inside JavaScript portable.

## Develop with hot reload

Run the server and Blitsen in separate terminals:

```sh
npm run dev
```

```sh
npx blitsen http://localhost:5173
```

If Vite's HMR client cannot infer its socket address, set `server.hmr.host` and `clientPort` in
`vite.config.js`. Inline and served external source maps remap uncaught runtime diagnostics;
`error.stack` inspected by application code remains the engine's generated stack.

## Include runtime-loaded files

Files reached only through computed names are not visible to the collector. Include them explicitly:

```sh
npx blitsen build dist \
  --include 'locales/**' \
  --include 'models/*.bin'
```

Then address them relative to the application or importing module. The build reports files it
omits so the include list can stay intentional.

## Fetch application data

Ship local data alongside the application and use a relative URL:

```js
const response = await fetch("./data/settings.json");
const settings = await response.json();
```

Make sure the file is statically referenced or matched by `--include`. A literal missing path is a
doctor error. Remote `fetch()` is supported, but remote scripts, modules, stylesheets and images
have narrower behavior; consult [Local and remote
resources](WEB-APIS.md#local-and-remote-resources) before relying on them.

## Persistent application data

`localStorage` is process-memory only in this release. The standard runtime does not expose a
general filesystem API, so durable state currently requires a native addon. Use `blitsen/app` to
choose the platform-appropriate directory:

```js
import app from "blitsen/app";

const directory = app.dataDir?.("MyApp");
if (!directory) {
  throw new Error("Application data directories are unavailable");
}
```

The helper returns a path and does not create it. The addon must create the directory and perform
the reads/writes. Carrying a `.node` addon selects the larger Bun host and changes packaging
obligations, so keep that tradeoff explicit.

## Use a file dialog with a fallback

Dialogs are Linux-only in this release, so keep an alternative interaction:

```js
import dialog from "blitsen/dialog";

export async function chooseProject() {
  if (!dialog.openFile) return null;
  return dialog.openFile({
    title: "Open project",
    filters: [{ name: "Project", extensions: ["json"] }],
  });
}
```

Grade each release target as well as testing the member at runtime:

```sh
npx blitsen doctor dist --target linux-x64
npx blitsen doctor dist --target darwin-arm64
npx blitsen doctor dist --target win32-x64
```

## Build a patchable asset layout

Use side-loaded assets when content needs to change without relinking the executable:

```sh
npx blitsen build dist --assets side-loaded --out MyApp
```

Distribute `MyApp` and `MyApp.assets/` together. On macOS the asset directory is placed inside the
application bundle beside its executable.

## Print third-party notices

The desktop export embeds the notices supplied by its runtime package:

```sh
./MyApp --licenses
```

On Windows:

```powershell
.\MyApp.exe --licenses
```

Keep this output available to recipients and read [Licensing](LICENSING.md) before distribution.

## Cross-build a desktop artifact

Choose one of the six target triples:

```sh
npx blitsen build dist --target win32-x64 --out MyApp.exe
```

Blitsen downloads the exact matching runtime package and caches it. Move the result to the target
platform for runtime testing and signing; cross-building does not prove that the UI behaves
correctly on that operating system.

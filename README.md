# Blitsen

> Write an app in HTML, CSS and TypeScript. Ship a native executable. No browser included.

Blitsen runs built HTML, CSS and JavaScript in a native window using
[Blitz](https://github.com/DioxusLabs/blitz) and an embedded JavaScript engine. It does not ship
Chromium or use the operating system WebView.

> **Pre-alpha:** Blitsen implements a deliberate subset of the web platform. Check your built
> application with `blitsen doctor`, test on every platform you ship, and expect breaking changes.

## Quick start

Install Blitsen in an existing web project:

```sh
npm install -D blitsen
```

Tell Blitsen how to produce and find the static build in `package.json`:

```json
{
  "scripts": {
    "native": "blitsen build"
  },
  "blitsen": {
    "build": "vite build",
    "output": "dist",
    "name": "My App"
  }
}
```

Open the built application in a native window:

```sh
npx blitsen
```

Check it against Blitsen's supported surface, then export it:

```sh
npx blitsen doctor dist
npm run native
```

The default desktop build produces a single executable containing the runtime and the reachable
application assets. For plain HTML with no build step, point the CLI directly at its directory:

```sh
npx blitsen .
npx blitsen build . --name "My App"
```

During development, Blitsen can replace the browser tab while Vite or another server continues to
handle transforms and hot reload:

```sh
npx blitsen http://localhost:5173
```

## What Blitsen expects

- A directory of **built static output** containing `index.html`. Blitsen does not transpile
  TypeScript, JSX, Vue or Svelte source and does not resolve bare npm imports at runtime.
- An application designed for Blitsen's web-platform subset. Missing APIs are absent so normal
  feature detection works; `blitsen doctor` reports references it can identify.
- Trusted application code. A Blitsen application is native software: there is no browser sandbox,
  same-origin policy or permission prompt.
- Local application UI, not arbitrary third-party websites.

Desktop runtimes are published for Linux, macOS and Windows on x64 and arm64. Android APK output is
available from a source checkout and requires the Android/Rust toolchain. See
[platform support](docs/PLATFORM-SUPPORT.md) before distributing an application.

## Documentation

| If you want to… | Read |
| --- | --- |
| Run your first application | [Getting started](docs/GETTING-STARTED.md) |
| Browse runnable source examples | [Examples](examples/README.md) |
| Understand what Blitsen loads | [Core concepts](docs/CORE-CONCEPTS.md) |
| Configure a project | [Configuration](docs/CONFIGURATION.md) |
| Use dialogs, the clipboard, window controls or OS data | [Native APIs](docs/NATIVE-APIS.md) |
| Solve common integration tasks | [Recipes](docs/RECIPES.md) |
| Create and distribute an executable or APK | [Packaging and distribution](docs/PACKAGING.md) |
| Check operating-system requirements and known limits | [Platform support](docs/PLATFORM-SUPPORT.md) |
| Look up every command and option | [CLI reference](docs/CLI.md) |
| Diagnose a failure | [Troubleshooting](docs/TROUBLESHOOTING.md) |

See [Web API support](docs/WEB-APIS.md) for the runtime boundary and the exact generated matrix.

## Licence and attribution

Blitsen source is dual-licensed under Apache-2.0 or MIT. Exported applications contain third-party
components with their own terms; read [Licensing](docs/LICENSING.md) before distribution and use
`./MyApp --licenses` to inspect the notices embedded in an export.

Blitsen is an independent project built on Blitz. It is not an official DioxusLabs project and is
not endorsed by DioxusLabs.

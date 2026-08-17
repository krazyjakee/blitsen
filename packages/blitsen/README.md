# Blitsen

Build a native application from static HTML, CSS and JavaScript without Chromium or an operating-
system WebView.

> Blitsen is pre-alpha and implements a subset of the web platform. Run `blitsen doctor` and test
> every target before distribution.

## Install

```sh
npm install -D blitsen
```

The package installs the CLI and the prebuilt runtime for the current Linux, macOS or Windows
platform. Installation does not compile Rust.

## Run an application

Open a directory containing built `index.html`:

```sh
npx blitsen dist
```

Or use a running development server and keep its transforms and hot reload:

```sh
npx blitsen http://localhost:5173
```

Blitsen accepts built static output. It does not transpile TypeScript/JSX/framework source or
resolve bare npm imports at runtime.

## Configure a project

Put the build command and output directory in `package.json`:

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

Then run without a directory:

```sh
npx blitsen
npx blitsen doctor dist
npm run native
```

The default build embeds reachable assets in one desktop executable. A directory argument such as
`blitsen build dist` skips the configured build command.

## Use native capabilities

Import native modules through normal package subpaths and feature-detect optional members:

```js
import clipboard from "blitsen/clipboard";

if (clipboard.writeText) {
  clipboard.writeText("Hello from Blitsen");
}
```

This release provides modules for application directories/lifecycle, window controls, Linux
dialogs, clipboard formats and operating-system readings. TypeScript declarations ship with the
package.

## Documentation

- [Getting started](https://blitsen.dev/docs/getting-started/)
- [Configuration](https://blitsen.dev/docs/configuration/)
- [Native APIs](https://blitsen.dev/docs/native-apis/)
- [Packaging and distribution](https://blitsen.dev/docs/packaging/)
- [CLI reference](https://blitsen.dev/docs/cli/)
- [Web API support](https://blitsen.dev/docs/web-apis/)
- [Troubleshooting](https://blitsen.dev/docs/troubleshooting/)

Run `npx blitsen --help` for every option. The source repository and issue tracker are at
[github.com/krazyjakee/blitsen](https://github.com/krazyjakee/blitsen).

Blitsen is independently built on [Blitz](https://github.com/DioxusLabs/blitz). It is not an
official DioxusLabs project and is not endorsed by DioxusLabs.

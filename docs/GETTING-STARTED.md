# Getting started

Install Blitsen, open a built web application in a native window, check compatibility and export a
desktop executable.

> Blitsen is pre-alpha and implements a subset of the web platform. Use `doctor` before every
> release and test the result on every operating system you plan to support.

## Prerequisites

You need Node.js and a package manager to install and run the CLI. Installing `blitsen` downloads a
prebuilt runtime for the current desktop platform; it does not compile Rust or run a post-install
build.

Blitsen accepts static web output with an `index.html`. If your project uses TypeScript, JSX, Vue,
Svelte or bare npm imports, keep using its existing build tool. Blitsen consumes the directory that
tool produces.

## Install Blitsen

From your project directory:

```sh
npm install -D blitsen
```

The same commands work through another package manager's equivalent executor.

## Try a plain HTML application

Create a directory containing `index.html`:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Hello</title>
    <style>
      body { display: grid; min-height: 100vh; place-items: center; margin: 0; }
    </style>
  </head>
  <body>
    <button id="hello">Say hello</button>
    <script>
      document.querySelector("#hello").addEventListener("click", (event) => {
        event.currentTarget.textContent = "Hello from Blitsen";
      });
    </script>
  </body>
</html>
```

Open the directory in a native window:

```sh
npx blitsen .
```

Use `--width`, `--height` and `--title` to change the development window:

```sh
npx blitsen . --width 1024 --height 720 --title "Hello"
```

## Add Blitsen to an existing project

Add a `blitsen` object to `package.json`. The `build` command can be any command that writes static
output; `output` is that directory, relative to `package.json`.

```json
{
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "native": "blitsen build"
  },
  "blitsen": {
    "build": "vite build",
    "output": "dist",
    "name": "My App"
  }
}
```

Run Blitsen without a directory to execute the configured build and open its output:

```sh
npx blitsen
```

Passing a directory bypasses the configured build command:

```sh
npm run build
npx blitsen dist
```

See [Configuration](CONFIGURATION.md) for every supported key.

## Use your development server

Start your usual server, then point Blitsen at it:

```sh
npm run dev
npx blitsen http://localhost:5173
```

The server continues to transform source and provide hot reload. Blitsen supplies the window and
runtime. A local-directory run watches built files too: CSS changes are swapped when possible and
other changes reload the document.

Source maps are not currently applied to runtime stack traces. See [Develop with hot
reload](RECIPES.md#develop-with-hot-reload) for the current workflow.

## Check the built output

Run `doctor` against static output, not source code or a development-server URL:

```sh
npm run build
npx blitsen doctor dist
```

Errors identify output that cannot survive in the current runtime and block export. Warnings name
unsupported or narrower behavior that needs review. A warning can still represent a real failure
if your application calls the reported API without a fallback.

For CI or other tools, request JSON:

```sh
npx blitsen doctor dist --json
```

## Export a desktop application

With the configuration above:

```sh
npm run native
```

Or build a directory directly:

```sh
npx blitsen build dist --name "My App" --out MyApp
```

Embedded assets are the default, so the result is one executable. Run that artifact on the target
platform and exercise the complete application before distributing it.

## Next steps

- Read [Core concepts](CORE-CONCEPTS.md) before adapting a browser application.
- Use [Recipes](RECIPES.md) for assets, routing, native modules and persistent data.
- Read [Packaging and distribution](PACKAGING.md) before adding icons, signing or cross-building.
- Keep [Troubleshooting](TROUBLESHOOTING.md) nearby when a build or runtime refuses something.

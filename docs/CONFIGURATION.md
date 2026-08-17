# Configuration

Blitsen reads configuration from the `blitsen` key of the nearest `package.json`. There is no
separate configuration file.

## Example

```json
{
  "scripts": {
    "native": "blitsen build"
  },
  "blitsen": {
    "build": "vite build",
    "output": "dist",
    "name": "My App",
    "addons": ["native/physics.node"]
  }
}
```

Only `output` is required.

## Keys

| Key | Type | Meaning |
| --- | --- | --- |
| `output` | string | Static output directory relative to `package.json`; it must contain `index.html` |
| `build` | string | Command to run before ingesting `output` |
| `name` | string | Application name, default window title and default output filename |
| `addons` | string array | `.node` addons to carry, with paths relative to `package.json` |

Unknown keys and empty values are rejected instead of ignored.

## How configuration is found

Starting at the current working directory, Blitsen walks upward and uses the nearest `package.json`
that declares a `blitsen` key. The configured build command runs from the directory containing that
file. Its local `node_modules/.bin` is placed on `PATH`, like a package-manager script.

Running either command with no directory applies the configuration:

```sh
npx blitsen
npx blitsen build
```

A directory argument means "use this output as it is" and skips both configuration discovery and
the configured build command:

```sh
npx blitsen dist
npx blitsen build dist
```

`doctor` is always explicit because silently checking the wrong directory would be dangerous:

```sh
npx blitsen doctor dist
```

## Build commands

Blitsen passes `build` to the platform shell exactly as written and stops if it exits non-zero.
Keep the command deterministic and make sure it leaves a complete static application in `output`.

Examples:

```json
{ "blitsen": { "build": "vite build", "output": "dist" } }
```

```json
{ "blitsen": { "build": "npm run build:web", "output": "public" } }
```

Blitsen does not inspect or configure Vite, webpack, Rollup or another builder.

## Native addons

Declare native Node-API addons that live outside the static output directory:

```json
{
  "blitsen": {
    "output": "dist",
    "addons": ["native/greet.node"]
  }
}
```

An addon selects the larger Bun-based host because the standard runtime does not implement
Node-API. It also changes the redistribution obligations; see [Native addons](PACKAGING.md#native-addons).
For a one-off build, repeat `--addon` instead.

## Schema and JavaScript validation

The JSON Schema Blitsen validates is published as `blitsen/config.schema.json`. JavaScript tooling
can validate the same object with `defineConfig`:

```js
import { defineConfig } from "blitsen";

const config = defineConfig({
  build: "vite build",
  output: "dist",
  name: "My App",
});
```

For `package.json`, use an editor schema association if you want completion for the nested
`blitsen` object. The CLI always validates before it runs the build command.

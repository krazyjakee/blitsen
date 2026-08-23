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
    "addons": ["native/physics.node"],
    "window": { "type": "borderless", "resizable": false },
    "tray": {
      "icon": "native/tray.png",
      "tooltip": "My App",
      "closeToTray": true,
      "contextMenu": [
        { "id": "open-report", "label": "Open report", "icon": "native/open.png" },
        { "type": "checkbox", "id": "launch", "label": "Launch at login", "checked": true },
        { "type": "submenu", "label": "Theme", "menu": [
          { "type": "radio", "id": "light", "label": "Light", "group": "theme", "checked": true },
          { "type": "radio", "id": "dark", "label": "Dark", "group": "theme" }
        ] },
        { "type": "separator" },
        { "label": "Quit", "action": "quit" }
      ]
    },
    "menu": {
      "menu": [
        { "type": "submenu", "role": "application", "label": "My App", "menu": [
          { "type": "role", "role": "about" },
          { "type": "separator" },
          { "type": "role", "role": "quit" }
        ] },
        { "type": "submenu", "label": "File", "menu": [
          { "id": "new", "label": "New", "accelerator": "CmdOrCtrl+KeyN" }
        ] }
      ]
    }
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
| `window` | object | Native window type and creation options |
| `tray` | object | System tray icon and context menu |
| `menu` | object | Application menu installed at startup; needs no tray icon |

Unknown keys and empty values are rejected instead of ignored.

## Native window and tray

`window.type` accepts `normal` (the default), `borderless`, `fullscreen`, or `hidden`.
The same object can set `resizable`, `transparent`, and `alwaysOnTop`. A hidden window requires a
tray configuration so the application is not launched without a way to reveal it.

`tray.icon` is a PNG path relative to `package.json`. Blitsen carries it into standalone exports.
`openOnClick` defaults to true. The optional `contextMenu` accepts built-in `show`, `hide`, and
`quit` actions; application-defined action IDs; separators; checkboxes; consecutive radio groups;
and nested submenus. Actions can set `enabled`, an `accelerator`, and a PNG `icon`; submenus can
also have a PNG icon. Checkable-item icons are omitted because the supported native menu backends
cannot represent them consistently. The legacy `{ "action": "separator" }` spelling remains valid.

IDs must be unique across the complete tree, menus may be at most 16 levels and 512 entries, and
each consecutive radio group starts with exactly one checked item. `closeToTray` requires a `quit`
action anywhere in the tree. Custom and checkable selections are delivered through
`blitsen/tray.onAction()` after application listeners install; built-in actions run in the native
session directly.

Every tray and menu icon path is relative to the `package.json` that declared the configuration.
Absolute paths and paths escaping that package are rejected. Blitsen validates the PNGs and carries
them under deterministic reserved names in embedded and side-loaded exports, including icons whose
source files are outside the static output directory.

## Application menu

`menu.menu` is the application menu installed before application JavaScript runs. It is separate
from `tray` because it needs no tray icon, and `blitsen/menu.configure()` replaces this same menu
rather than adding a second one — startup configuration and runtime replacement address one object.

Every top-level entry is a submenu, because that is what a menu bar holds. Below that the tree
follows the tray's rules — nested submenus, application-defined action IDs, checkboxes, consecutive
radio groups, separators and accelerators, with IDs unique across the whole tree and at most 16
levels and 512 entries. Two things differ: `{ "type": "role", "role": "copy" }` items carry
platform commands the tray has no use for, and there are no icons, because a macOS main menu shows
none.

A top-level submenu may also declare `"role"`: `application`, `edit`, `window` or `help`, at most
one of each. On macOS that claims a position AppKit reads positionally rather than by title, and
Blitsen supplies a standard submenu for each of `application`, `edit` and `window` that the
application did not claim. See [Application menu](NATIVE-APIS.md#application-menu) for the ordering
rules and [PLATFORM-SUPPORT.md](PLATFORM-SUPPORT.md#application-menu) for where a menu exists at
all. A configured menu on a platform without one is validated and then installs nothing; it is not
an error, because the same configuration has to build for every target.

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

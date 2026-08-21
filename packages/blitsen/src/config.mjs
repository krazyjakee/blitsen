import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { delimiter, dirname, join, resolve } from "node:path";

// One canonical location: the "blitsen" key of package.json. This object is the
// contract, validated against below and published verbatim as
// config.schema.json for editor completion, so the two cannot drift.
export const CONFIG_SCHEMA = {
  $schema: "http://json-schema.org/draft-07/schema#",
  $id: "https://raw.githubusercontent.com/krazyjakee/blitsen/main/packages/blitsen/src/config.schema.json",
  title: "Blitsen configuration",
  description: "The \"blitsen\" key of package.json: build and static-output settings plus "
    + "native desktop presentation options.",
  type: "object",
  additionalProperties: false,
  required: ["output"],
  allOf: [
    {
      if: {
        required: ["window"],
        properties: { window: { required: ["type"], properties: { type: { const: "hidden" } } } },
      },
      then: { required: ["tray"] },
    },
    {
      if: {
        required: ["tray"],
        properties: {
          tray: { required: ["closeToTray"], properties: { closeToTray: { const: true } } },
        },
      },
      then: {
        properties: {
          tray: {
            required: ["contextMenu"],
            properties: {
              contextMenu: {
                contains: { required: ["action"], properties: { action: { const: "quit" } } },
              },
            },
          },
        },
      },
    },
  ],
  properties: {
    build: {
      type: "string",
      minLength: 1,
      description: "Command run before ingest, from the directory holding this package.json. "
        + "Blitsen runs it and consumes its output directory; it never inspects or configures "
        + "the build tool.",
    },
    output: {
      type: "string",
      minLength: 1,
      description: "Directory of static web output, relative to this package.json. "
        + "It must contain index.html.",
    },
    name: {
      type: "string",
      minLength: 1,
      description: "Application name. Sets the native window title and the default output file name.",
    },
    addons: {
      type: "array",
      items: { type: "string", minLength: 1 },
      description: "Native .node addons carried into the export, relative to this package.json. "
        + "They live outside the output directory more often than not, so ingest cannot reach "
        + "them and they have to be declared.",
    },
    window: {
      type: "object",
      additionalProperties: false,
      description: "Native window creation options.",
      properties: {
        type: {
          type: "string",
          enum: ["normal", "borderless", "fullscreen", "hidden"],
          description: "Initial native window presentation. Defaults to normal.",
        },
        resizable: { type: "boolean", description: "Whether the user can resize the window." },
        transparent: {
          type: "boolean",
          description: "Request a transparent native surface. Support depends on the compositor.",
        },
        alwaysOnTop: {
          type: "boolean",
          description: "Request that the window stay above normal windows.",
        },
      },
    },
    tray: {
      type: "object",
      additionalProperties: false,
      required: ["icon"],
      description: "System tray icon and its built-in context-menu actions.",
      properties: {
        icon: {
          type: "string",
          minLength: 1,
          pattern: "\\.png$",
          description: "PNG tray icon, relative to this package.json.",
        },
        tooltip: { type: "string", minLength: 1 },
        openOnClick: {
          type: "boolean",
          description: "Show and focus the window when the tray icon is activated. Defaults to true.",
        },
        closeToTray: {
          type: "boolean",
          description: "Hide the window instead of exiting when its close control is used.",
        },
        contextMenu: {
          type: "array",
          items: {
            type: "object",
            additionalProperties: false,
            required: ["action"],
            properties: {
              action: { type: "string", enum: ["show", "hide", "quit", "separator"] },
              label: { type: "string", minLength: 1 },
              enabled: { type: "boolean" },
            },
          },
          description: "Ordered show, hide, quit and separator entries for the tray context menu.",
        },
      },
    },
  },
};

function describeType(value) {
  if (value === null) return "null";
  if (Array.isArray(value)) return "an array";
  return `a ${typeof value}`;
}

export function validateConfig(config, source) {
  const fail = (detail) => { throw new Error(`invalid blitsen config in ${source}: ${detail}`); };
  const known = Object.keys(CONFIG_SCHEMA.properties);
  if (typeof config !== "object" || config === null || Array.isArray(config)) {
    fail(`expected an object, found ${describeType(config)}`);
  }
  for (const key of Object.keys(config)) {
    if (!known.includes(key)) fail(`unknown key "${key}" (known keys: ${known.join(", ")})`);
  }
  for (const key of CONFIG_SCHEMA.required) {
    if (!(key in config)) fail(`missing required key "${key}"`);
  }
  const checkValue = (label, value, rule) => {
    if (rule.type === "array") {
      if (!Array.isArray(value)) fail(`${label} must be an array, found ${describeType(value)}`);
      value.forEach((item, index) => checkValue(`${label.slice(0, -1)}[${index}]"`, item, rule.items));
      return;
    }
    if (rule.type === "object") {
      if (typeof value !== "object" || value === null || Array.isArray(value)) {
        fail(`${label} must be an object, found ${describeType(value)}`);
      }
      const objectKeys = Object.keys(rule.properties);
      for (const key of Object.keys(value)) {
        if (!objectKeys.includes(key)) {
          fail(`${label.slice(0, -1)}.${key}" is unknown (known keys: ${objectKeys.join(", ")})`);
        }
      }
      for (const key of rule.required ?? []) {
        if (!(key in value)) fail(`${label} is missing required key "${key}"`);
      }
      for (const [key, childRule] of Object.entries(rule.properties)) {
        if (key in value) checkValue(`${label.slice(0, -1)}.${key}"`, value[key], childRule);
      }
      return;
    }
    if (typeof value !== rule.type) {
      fail(`${label} must be a ${rule.type}, found ${describeType(value)}`);
    }
    if (rule.minLength !== undefined && value.trim().length < rule.minLength) {
      fail(`${label} must not be empty`);
    }
    if (rule.enum && !rule.enum.includes(value)) {
      fail(`${label} must be one of ${rule.enum.join(", ")}, found ${JSON.stringify(value)}`);
    }
    if (rule.pattern && !(new RegExp(rule.pattern)).test(value)) {
      fail(`${label} must match ${rule.pattern}`);
    }
  };
  for (const [key, rule] of Object.entries(CONFIG_SCHEMA.properties)) {
    if (key in config) checkValue(`"${key}"`, config[key], rule);
  }
  if (config.window?.type === "hidden" && !config.tray) {
    fail('"window.type" hidden requires a "tray" configuration');
  }
  if (config.tray?.closeToTray
    && !config.tray.contextMenu?.some(item => item.action === "quit")) {
    fail('"tray.closeToTray" requires a quit action in "tray.contextMenu"');
  }
  return config;
}

export function defineConfig(config) {
  return validateConfig(config, "defineConfig()");
}

// The nearest package.json declaring the key wins; the nearest one overall is
// remembered so "no config" can name the file the user probably meant.
export async function loadConfig(from = process.cwd()) {
  let directory = resolve(from);
  let nearest = null;
  for (;;) {
    const path = join(directory, "package.json");
    const source = await readFile(path, "utf8").catch(() => null);
    if (source !== null) {
      nearest ??= path;
      let manifest;
      try {
        manifest = JSON.parse(source);
      } catch (error) {
        throw new Error(`${path} is not valid JSON: ${error.message}`);
      }
      if (manifest.blitsen !== undefined) {
        return { path, root: directory, config: validateConfig(manifest.blitsen, path) };
      }
    }
    const parent = dirname(directory);
    if (parent === directory) return { path: nearest, root: null, config: null };
    directory = parent;
  }
}

// Structural constraint 6: this runs the command the user configured and consumes
// a directory. Blitsen never inspects the build tool, so the command is handed to
// the platform shell exactly as written, with the local .bin on PATH the way a
// package-manager script would see it.
export function runBuildCommand(command, cwd) {
  // Windows spells it Path, and a spread of process.env is a plain object, so the
  // existing key has to be replaced rather than a second one added.
  const key = Object.keys(process.env).find(name => name.toUpperCase() === "PATH") ?? "PATH";
  const env = {
    ...process.env,
    [key]: [join(cwd, "node_modules", ".bin"), process.env[key] ?? ""].join(delimiter),
  };
  return new Promise((settle, reject) => {
    const child = spawn(command, { cwd, shell: true, stdio: "inherit", env });
    child.on("error", error =>
      reject(new Error(`build command could not start: ${command} (${error.message})`)));
    child.on("close", (code, signal) => {
      if (signal) reject(new Error(`build command was terminated by ${signal}: ${command}`));
      else if (code !== 0) reject(new Error(`build command failed with exit code ${code}: ${command}`));
      else settle();
    });
  });
}

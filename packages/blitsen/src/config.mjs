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
  description: "The \"blitsen\" key of package.json: the command Blitsen runs before ingest, "
    + "and the directory of static web output it ingests.",
  type: "object",
  additionalProperties: false,
  required: ["output"],
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
  for (const [key, rule] of Object.entries(CONFIG_SCHEMA.properties)) {
    if (!(key in config)) continue;
    const value = config[key];
    if (typeof value !== rule.type) fail(`"${key}" must be a ${rule.type}, found ${describeType(value)}`);
    if (value.trim().length < rule.minLength) fail(`"${key}" must not be empty`);
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

import { access, realpath } from "node:fs/promises";
import { constants } from "node:fs";
import { join, relative, resolve } from "node:path";

const HELP = `Usage: blitsen <directory> [options]

Open <directory>/index.html in a native Blitsen window.

Options:
  --width <pixels>   Initial logical width (default: 800)
  --height <pixels>  Initial logical height (default: 600)
  --title <text>     Native window title (default: Blitsen)
  -h, --help         Show help
  -v, --version      Show version`;

export function parseArgs(args) {
  if (args.length === 0 || args.includes("--help") || args.includes("-h")) {
    return { help: true };
  }
  if (args.includes("--version") || args.includes("-v")) {
    return { version: true };
  }
  const options = { directory: null, width: 800, height: 600, title: "Blitsen" };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (["--width", "--height", "--title"].includes(argument)) {
      const value = args[++index];
      if (value === undefined) throw new Error(`${argument} requires a value`);
      if (argument === "--title") options.title = value;
      else {
        const pixels = Number(value);
        if (!Number.isInteger(pixels) || pixels <= 0)
          throw new Error(`${argument} must be a positive integer`);
        options[argument.slice(2)] = pixels;
      }
    } else if (argument.startsWith("-")) {
      throw new Error(`unknown option: ${argument}`);
    } else if (options.directory === null) {
      options.directory = argument;
    } else {
      throw new Error(`unexpected argument: ${argument}`);
    }
  }
  if (options.directory === null) throw new Error("missing application directory");
  return options;
}

export async function resolveApplication(directory) {
  const root = await realpath(resolve(directory)).catch(() => {
    throw new Error(`application directory does not exist: ${directory}`);
  });
  const entrypoint = join(root, "index.html");
  await access(entrypoint, constants.R_OK).catch(() => {
    throw new Error(`missing or unreadable entrypoint: ${entrypoint}`);
  });
  return { root, entrypoint };
}

export async function resolveAsset(root, fromFile, specifier) {
  if (specifier.startsWith("/") || /^[a-z][a-z\d+.-]*:/i.test(specifier)) {
    throw new Error(`asset URL must be relative to its document: ${specifier}`);
  }
  const asset = await realpath(resolve(fromFile, "..", specifier)).catch(() => {
    throw new Error(`unreadable asset: ${specifier}`);
  });
  const outside = relative(root, asset);
  if (outside === ".." || outside.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`)) {
    throw new Error(`asset escapes the application directory: ${specifier}`);
  }
  await access(asset, constants.R_OK);
  return asset;
}

export async function main(args, output = console, runtime = null) {
  try {
    const options = parseArgs(args);
    if (options.help) {
      output.log(HELP);
      return 0;
    }
    if (options.version) {
      output.log("0.0.1");
      return 0;
    }
    const application = await resolveApplication(options.directory);
    if (!runtime?.openDirectory) {
      throw new Error("native addon is unavailable; reinstall blitsen for this platform");
    }
    await runtime.openDirectory({ ...application, ...options });
    return 0;
  } catch (error) {
    output.error(`blitsen: ${error.message}`);
    return 1;
  }
}

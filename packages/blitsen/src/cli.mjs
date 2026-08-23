import { spawn } from "node:child_process";
import { access, realpath } from "node:fs/promises";
import { constants, watch as watchFs } from "node:fs";
import { extname, join, normalize, resolve } from "node:path";
import { ANDROID_ABIS, androidNotices, buildAndroid, DEFAULT_ABIS } from "./android.mjs";
import { ASSET_ROOT } from "./android-assets.mjs";
import { loadConfig, recordTrayConfiguration, runBuildCommand } from "./config.mjs";
import { doctorApplication, formatDiagnostic } from "./doctor.mjs";
import { buildStandalone } from "./export.mjs";
import { frameDelay } from "./frame-pacing.mjs";
import { ANDROID_TARGETS } from "./native-modules.mjs";
import { developmentBundle, developmentIdentifier, signArtifact } from "./packaging.mjs";
import { describeRuntime, hostTarget, openRuntime, packageVersion, resolveRuntime,
  runtimeCacheDir, TARGETS } from "./runtime.mjs";

const HELP = `Usage: blitsen [directory|url] [options]
       blitsen build [directory] [options]
       blitsen doctor <directory> [--target <triple>] [--json]

Open <directory>/index.html in a native Blitsen window. Given an http(s) URL —
blitsen http://localhost:5173 — the window reads the document and its modules
from your own dev server instead, so HMR, source maps and the rest of your
inner loop keep working.
With no directory, running and building do the same thing to find one: read the
"blitsen" config in package.json, run the configured build command, and take
its output directory — or, where there is no config, the directory you are
standing in.
Build creates a single-file executable: Blitsen's own runtime with the
application appended to it. With --android it creates a signed APK instead,
cross-compiled for every ABI asked for, with the application under assets/.
Doctor checks built static output against the v1 compatibility profile, and
against the native: modules the target it is grading for actually has.

Options:
  --width <pixels>   Initial logical width (default: 800)
  --height <pixels>  Initial logical height (default: 600)
  --title <text>     Native window title (default: the application name)
  --name <text>      Application name: window title and default output name
  --out <path>       Build output path (default: the application name)
  --outfile <path>   Alias of --out
  --target <triple>  Build for another platform; its runtime is fetched and cached.
                     With doctor, grade against that platform instead of this one
  --include <glob>   Keep an unreferenced output file (repeatable)
  --addon <path>     Carry a .node addon into the export (repeatable)
  --accept-errors    Export despite compatibility errors, accepting what they cost
  --assets <layout>  embedded (default) or side-loaded next to the executable
  --icon <path>      Application icon: PNG, or a platform-native .ico/.icns/.svg
  --bundle-id <id>   Application identity: the macOS CFBundleIdentifier (default:
                     com.blitsen.<title>), and the identity a notification
                     activation is addressed to, which has no default
  --app-version <v>  Application version recorded in the platform metadata
  --sign <command>   Signing hook, run with the packaged artifact as its argument
  --dev-bundle       macOS, run only: build a signed development .app around this
                     interpreter and run inside it, so the development host has a
                     notification identity of its own. --bundle-id names it;
                     --sign replaces the ad-hoc signature
  --force            Replace an existing build output
  --json             Emit the doctor report as JSON
  -h, --help         Show help
  -v, --version      Show version

Android (build only; an APK is a cross-compile, not a runtime an install
resolves, so it is a flag rather than a --target value):
  --android              Build an APK instead of a desktop executable
  --android-abi <abi>    ABI to include, repeatable (default: ${DEFAULT_ABIS.join(", ")};
                         also ${Object.keys(ANDROID_ABIS).filter(abi => !DEFAULT_ABIS.includes(abi)).join(", ")})
  --android-package <id> Application ID (default: com.blitsen.<name>)
  --android-keystore <p> Sign with this keystore; its password is read from
                         BLITSEN_ANDROID_KEYSTORE_PASSWORD, never from a flag.
                         BLITSEN_ANDROID_KEY_ALIAS and
                         BLITSEN_ANDROID_KEY_PASSWORD name the key inside it
                         when the store holds more than one. Without a keystore
                         the APK is signed with the Android debug key:
                         installable, not distributable
  --android-debug        Build the debug profile — unoptimised Rust, and a
                         manifest marked debuggable. Every entry in the APK is
                         stored uncompressed on both profiles`;

// The resolver owns it now, because the version pin is checked there; still on this
// module's surface, which is where callers ask for it.
export { packageVersion };

const PACKAGE_OPTIONS = { "--icon": "icon", "--bundle-id": "bundleId", "--app-version": "appVersion", "--sign": "sign" };
// The Android artifact's own options (#148). Separate from PACKAGE_OPTIONS
// because they describe a different artifact rather than more metadata on the
// same one: an application ID is not a CFBundleIdentifier under another name —
// it is the key an install is tracked by and cannot be changed after release.
const ANDROID_OPTIONS = {
  "--android-package": "androidPackage", "--android-keystore": "androidKeystore",
};
const BUILD_OPTIONS = ["--out", "--outfile", "--name", "--target", "--include", "--addon", "--assets",
  "--android-abi", ...Object.keys(ANDROID_OPTIONS), ...Object.keys(PACKAGE_OPTIONS)];
const VALUE_OPTIONS = ["--width", "--height", "--title", ...BUILD_OPTIONS];
// A build-only switch: doctor's own exit code must keep meaning what it says.
const BUILD_FLAGS = ["--accept-errors", "--android", "--android-debug"];
// Everything that only means something once --android has been asked for. Named
// so that `--android-abi x86_64` without `--android` is refused rather than
// silently building a desktop executable that ignored it.
const ANDROID_ONLY = ["--android-abi", "--android-debug", ...Object.keys(ANDROID_OPTIONS)];
// The one build option doctor also takes, because doctor grades against a
// target rather than for one: which `native:` modules exist is a property of the
// platform, and asking about a platform is not the same as claiming to build for
// it (#147).
const DOCTOR_OPTIONS = ["--target"];
// The two packaging options a run also takes, and only inside a development
// bundle (#253): which identity that bundle carries, and how it is signed. The
// pairing is checked after the loop rather than at the flag, because
// `--dev-bundle` may be typed after either of them — the same reason the
// Android options are checked there.
const DEV_BUNDLE_OPTIONS = { "--bundle-id": "bundleId", "--sign": "sign" };
// TECH.md §11: one binary package per target (src/runtime.mjs). A cross-target
// build links that target's runtime, fetched on demand (#72), and compiles the
// launcher for that target's Bun. What it cannot do is sign or notarise for a
// platform it is not running on — see the note in the build path.
//
// Doctor accepts more than build does, and deliberately: it reads files and
// links nothing, so it can answer for Android, which has no runtime package to
// resolve and is not a P5b row at all (PRODUCT.md P5c, #148). Building for one
// is a different claim and is still refused.
function checkTarget(value, command) {
  const allowed = command === "doctor" ? [...TARGETS, ...ANDROID_TARGETS] : TARGETS;
  if (!allowed.includes(value)) {
    throw new Error(`unknown --target ${value} (expected one of: ${allowed.join(", ")})`);
  }
}

export function parseArgs(args) {
  if (args.includes("--help") || args.includes("-h")) {
    return { help: true };
  }
  if (args.includes("--version") || args.includes("-v")) {
    return { version: true };
  }
  const command = ["build", "doctor"].includes(args[0]) ? args[0] : "run";
  const options = { command, directory: null, width: 800, height: 600, title: "Blitsen" };
  for (let index = command === "run" ? 0 : 1; index < args.length; index += 1) {
    const argument = args[index];
    if (VALUE_OPTIONS.includes(argument)) {
      const value = args[++index];
      if (value === undefined) throw new Error(`${argument} requires a value`);
      if (command === "doctor" && !DOCTOR_OPTIONS.includes(argument)) {
        throw new Error(`${argument} is not valid with doctor`);
      }
      if (BUILD_OPTIONS.includes(argument) && command === "run"
        && DEV_BUNDLE_OPTIONS[argument] === undefined) {
        throw new Error(`${argument} is only valid with build`
          + `${DOCTOR_OPTIONS.includes(argument) ? " or doctor" : ""}`);
      }
      if (PACKAGE_OPTIONS[argument]) options[PACKAGE_OPTIONS[argument]] = value;
      else if (ANDROID_OPTIONS[argument]) options[ANDROID_OPTIONS[argument]] = value;
      else if (argument === "--android-abi") {
        if (!Object.keys(ANDROID_ABIS).includes(value)) {
          throw new Error(`unknown --android-abi ${value} `
            + `(expected one of: ${Object.keys(ANDROID_ABIS).join(", ")})`);
        }
        options.androidAbis = [...options.androidAbis ?? [], value];
      }
      else if (argument === "--title") options.title = value;
      else if (argument === "--name") options.name = value;
      else if (argument === "--out" || argument === "--outfile") options.outfile = value;
      else if (argument === "--target") {
        checkTarget(value, command);
        options.target = value;
      }
      else if (argument === "--include") options.include = [...options.include ?? [], value];
      // Resolved here rather than in the exporter: the path is the user's, and it
      // usually points outside the directory being ingested.
      else if (argument === "--addon") options.addons = [...options.addons ?? [], resolve(value)];
      else if (argument === "--assets") {
        if (!["embedded", "side-loaded"].includes(value))
          throw new Error("--assets must be embedded or side-loaded");
        options.assets = value;
      }
      else {
        const pixels = Number(value);
        if (!Number.isInteger(pixels) || pixels <= 0)
          throw new Error(`${argument} must be a positive integer`);
        options[argument.slice(2)] = pixels;
      }
    } else if (argument === "--force") {
      if (command !== "build") throw new Error("--force is only valid with build");
      options.force = true;
    } else if (BUILD_FLAGS.includes(argument)) {
      if (command !== "build") throw new Error(`${argument} is only valid with build`);
      if (argument === "--android") options.android = true;
      else if (argument === "--android-debug") options.androidDebug = true;
      else options.acceptErrors = true;
    } else if (argument === "--dev-bundle") {
      if (command !== "run") throw new Error("--dev-bundle is only valid with run: a build "
        + "already produces a bundle, and --bundle-id and --sign describe that one");
      options.devBundle = true;
    } else if (argument === "--json") {
      if (command !== "doctor") throw new Error("--json is only valid with doctor");
      options.json = true;
    } else if (argument.startsWith("-")) {
      throw new Error(`unknown option: ${argument}`);
    } else if (options.directory === null) {
      options.directory = argument;
    } else {
      throw new Error(`unexpected argument: ${argument}`);
    }
  }
  // A run with no directory is left null for the same reason a build is: which
  // directory that means is the configuration's answer, not this function's, and
  // answering "." here is what kept `blitsen` from ever reading the config —
  // standing in a bundler project, it opened the source tree and found no
  // index.html while `blitsen build` beside it built and ingested one.
  // Doctor is pointed rather than guessed: it grades build output, and defaulting
  // it to wherever the shell happens to be would grade the wrong tree in silence.
  if (options.directory === null && options.command === "doctor") {
    throw new Error("missing application directory");
  }
  applyName(options);
  checkAndroidOptions(options);
  checkDevBundleOptions(options);
  return options;
}

// A run may name an identity and a signing command, but only for the artifact
// `--dev-bundle` produces. Without it there is no artifact, so accepting them
// silently would be the same failure `checkAndroidOptions` guards against: the
// flag was typed, the run started, and nothing was identified or signed.
function checkDevBundleOptions(options) {
  if (options.command !== "run" || options.devBundle) return;
  for (const [flag, field] of Object.entries(DEV_BUNDLE_OPTIONS)) {
    if (options[field] !== undefined) {
      throw new Error(`${flag} needs --dev-bundle when running: a run outside a development `
        + "bundle produces no artifact to identify or sign");
    }
  }
}

// What `--android` is compatible with, checked once rather than at each flag —
// the incompatibilities are between options, and a check written per flag reads
// as five unrelated rules instead of one decision.
//
// Every refusal here is a case where the desktop option describes a thing an
// APK does not have, so accepting it and ignoring it would be the failure mode
// `docs/PRODUCT.md` §7 exists to prevent: the flag was typed, the build
// succeeded, and the artifact does not do what was asked.
const ANDROID_FIELDS = { androidAbis: "--android-abi", androidDebug: "--android-debug",
  androidPackage: "--android-package", androidKeystore: "--android-keystore" };
function checkAndroidOptions(options) {
  if (!options.android) {
    for (const [field, flag] of Object.entries(ANDROID_FIELDS)) {
      if (options[field] !== undefined) throw new Error(`${flag} needs --android`);
    }
    return;
  }
  if (options.target !== undefined) {
    throw new Error("--target and --android name different artifacts: --target picks one of "
      + "the six desktop runtimes to link, and an APK links none of them and carries several "
      + "architectures at once. Choose the ABIs with --android-abi.");
  }
  if (options.assets !== undefined) {
    throw new Error("--assets is not valid with --android: an APK's files live in assets/ "
      + "inside the signed archive, and there is nothing beside it to side-load from.");
  }
  if (options.addons !== undefined) {
    throw new Error("--addon is not valid with --android: a carried .node addon is Node-API "
      + "and needs the Bun host, which does not exist on Android.");
  }
  if (options.icon !== undefined) {
    throw new Error("--icon is not valid with --android yet: an Android launcher icon is a "
      + "resource in several densities rather than one file beside the executable, and this "
      + "build generates no resource directory.");
  }
  // The two names are the same thing under two platforms' vocabularies — the
  // application's reverse-DNS identity — so one given without the other is
  // taken as meant. `--android-package` still wins where both are present,
  // because it is the more specific of the two.
  if (options.androidPackage === undefined && options.bundleId !== undefined) {
    options.androidPackage = options.bundleId;
  }
}

// The window title follows the application name unless --title says otherwise;
// the two are only distinguishable when the title is still its default.
function applyName(options) {
  if (options.name !== undefined && options.title === "Blitsen") options.title = options.name;
}

const STEPS = { build: "⓪", ingest: "①", scan: "②", collect: "③", link: "④", package: "⑤" };
const NOTE_INDENT = " ".repeat(10);

function reportStep(output, { step, detail, notes = [] }) {
  output.log(`${STEPS[step]} ${step.padEnd(7)} ${detail}`);
  for (const note of notes) output.log(`${NOTE_INDENT}${note}`);
}

// --out wins, then the application name, then the exporter's directory-name default.
function buildOutfile(options) {
  if (options.outfile !== undefined) return options.outfile;
  return options.name === undefined ? undefined : resolve(process.cwd(), options.name);
}

async function applyConfiguration(options, output) {
  const { path, root, config } = await loadConfig();
  if (!config) {
    // A directory of static output is already an application: there is no build
    // command to configure, and the directory you are standing in is the one you
    // meant — which is what `blitsen` with no argument has always opened, and is
    // kept here so that it still does. Only when there is nothing here to run
    // does the config matter, and then the message is about the config rather
    // than about the entrypoint — a bundler project whose config is missing must
    // not quietly run or export its source directory instead.
    const here = join(process.cwd(), "index.html");
    if (await access(here, constants.R_OK).then(() => true, () => false)) {
      options.directory = process.cwd();
      return;
    }
    const location = path ?? join(process.cwd(), "package.json");
    throw new Error("missing application directory: pass one, or add an index.html here, "
      + `or add a "blitsen" config to ${location}`);
  }
  if (options.android && (config.window || config.tray || config.menu)) {
    throw new Error("window, tray and menu configuration is only available to desktop builds");
  }
  if (config.build) {
    reportStep(output, { step: "build", detail: `${config.build} (configured in ${path})` });
    await runBuildCommand(config.build, root);
  }
  options.directory = resolve(root, config.output);
  options.addons = [...config.addons?.map(addon => resolve(root, addon)) ?? [], ...options.addons ?? []];
  options.name ??= config.name;
  options.window = config.window;
  options.tray = config.tray
    ? await recordTrayConfiguration(config.tray, root)
    : undefined;
  // Nothing to resolve: an application menu carries no assets, which is most of
  // why it travels as the tree the user wrote rather than a recorded one.
  options.menu = config.menu;
  applyName(options);
}

/** Whether the argument names a dev server rather than a directory (#67). */
export function isServerUrl(directory) {
  return typeof directory === "string" && /^https?:\/\//i.test(directory.trim());
}

export async function resolveApplication(directory) {
  // Proxy mode: nothing to resolve on this machine. The runtime asks the server
  // for the document, and says so clearly when nothing is answering yet.
  if (isServerUrl(directory)) {
    const url = directory.trim();
    return { root: url, entrypoint: url, served: true };
  }
  const root = await realpath(resolve(directory)).catch(() => {
    throw new Error(`application directory does not exist: ${directory}`);
  });
  const entrypoint = join(root, "index.html");
  await access(entrypoint, constants.R_OK).catch(() => {
    throw new Error(`missing or unreadable entrypoint: ${entrypoint}`);
  });
  return { root, entrypoint };
}

export function createReloadCoordinator(runtime, output = console, debounceMs = 100) {
  let pending = new Set();
  let timer = null;
  let closed = false;
  let reloads = Promise.resolve();
  const flush = () => {
    timer = null;
    const changed = pending;
    pending = new Set();
    if (changed.size === 0 || closed) return;
    reloads = reloads.then(async () => {
      if ([...changed].every(file => extname(file).toLowerCase() === ".css")) {
        // reloadCSS reports false when no <link rel=stylesheet> resolves to the
        // file: an @import target, or a sheet added since the document loaded.
        // Those still affect the render, so fall back to a document reload.
        let swapped = 0;
        for (const file of changed) swapped += await runtime.reloadCSS(file) ? 1 : 0;
        if (swapped === 0) await runtime.reloadDirectory();
      } else {
        await runtime.reloadDirectory();
      }
    }).catch(error => output.error(`blitsen: reload failed: ${error.message}`));
  };
  return {
    notify(file) {
      if (closed || !file) return;
      pending.add(normalize(String(file)));
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(flush, debounceMs);
    },
    close() {
      closed = true;
      pending.clear();
      if (timer !== null) clearTimeout(timer);
      timer = null;
    },
    settled() { return reloads; },
  };
}

export function watchApplication(root, runtime, output = console, debounceMs = 100) {
  const coordinator = createReloadCoordinator(runtime, output, debounceMs);
  const watcher = watchFs(root, { recursive: true }, (_event, file) => coordinator.notify(file));
  watcher.on("error", error => output.error(`blitsen: watcher failed: ${error.message}`));
  return {
    close() {
      watcher.close();
      coordinator.close();
    },
  };
}

// Resolve every host addon here so environment precedence, package versions and
// target compatibility are checked before native code is loaded.
async function hostRuntime(output) {
  const resolved = await resolveRuntime({ onNotice: message => output.error(message) });
  const waitForNextFrame = globalThis.Bun === undefined
    ? undefined
    : delay => globalThis.Bun.sleep(delay);
  return {
    ...openRuntime(resolved, { ...(waitForNextFrame ? { waitForNextFrame } : {}) }),
    build: options => buildStandalone(options, resolved),
  };
}

// A build for another target links that target's runtime, so it resolves its own
// rather than reusing the host's — and it is never opened, because it cannot run
// here. `fetch` is on for exactly this path: a cross-target build is the only
// one allowed to reach the network for a runtime it does not have.
async function targetRuntime(target, output) {
  if (target === undefined || target === hostTarget()) return hostRuntime(output);
  const resolved = await resolveRuntime({
    target, fetch: true, onNotice: message => output.error(message),
  });
  return { resolved, build: options => buildStandalone(options, resolved) };
}

// The Android artifact, reported the way a desktop one is.
//
// A separate function rather than another branch inside `main` because almost
// nothing is shared past step ②: there is no linked runtime to name, no host to
// choose, no side-loaded directory, and the three lines that follow the artifact
// are about signing and distribution rather than about a runtime version.
async function buildAndroidArtifact(options, application, output) {
  const notices = await androidNotices();
  const result = await buildAndroid({
    root: application.root,
    name: options.name ?? options.title,
    outfile: options.outfile,
    abis: options.androidAbis,
    applicationId: options.androidPackage ?? null,
    appVersion: options.appVersion ?? "0.1.0",
    keystore: options.androidKeystore ?? null,
    keystorePassword: process.env.BLITSEN_ANDROID_KEYSTORE_PASSWORD ?? null,
    // The two the keystore's own password does not cover: a store holding more
    // than one key, and a key whose password differs from the store's. Both in
    // the environment for the reason the keystore password is (android.mjs
    // decision 3), and both optional because neither is true of a keystore made
    // for one application.
    keyAlias: process.env.BLITSEN_ANDROID_KEY_ALIAS ?? null,
    keyPassword: process.env.BLITSEN_ANDROID_KEY_PASSWORD ?? null,
    release: !options.androidDebug,
    include: options.include ?? [],
    force: options.force ?? false,
    extra: notices === null ? new Map() : new Map([[notices.file, notices.contents]]),
    progress: event => reportStep(output, event),
    output,
  });
  const signed = options.sign
    ? await signArtifact({ command: options.sign, artifact: result.outfile })
    : null;
  if (signed) output.log(`Signed ${signed.artifact} with: ${signed.command}`);
  output.log(`Built ${result.outfile} (${result.assets} assets, ${result.bytes} bytes)`);
  output.log(`Android: ${result.applicationId} ${options.appVersion ?? "0.1.0"} `
    + `(versionCode ${result.versionCode}), ABIs ${result.abis.join(", ")}`);
  if (notices) {
    output.log(`Third-party notices: packaged, ${notices.bytes} bytes `
      + `(assets/${ASSET_ROOT}/${notices.file})`);
  } else {
    output.log("This APK is not cleared for redistribution: it carries no third-party notices, "
      + "and Android has no platform package to take them from — set BLITSEN_NOTICES_PATH "
      + "(docs/LICENSING.md).");
  }
  return 0;
}

// The bundle the current process is already running inside. The child is handed
// the same command line, `--dev-bundle` included, so something has to stop it
// building a bundle around a bundle; this is what says which side of the
// re-execution a process is on.
const DEV_BUNDLE = "BLITSEN_DEV_BUNDLE";

// Issue #253: macOS gates `UNUserNotificationCenter` on an application identity
// — a bundle identifier and a signature — and a development run is an
// interpreter executing a script, which has neither. The exported `.app` is the
// only Blitsen artifact that qualifies, so a developer could not exercise
// notification submission before shipping.
//
// So the development host is given an identity of its own: the same Info.plist
// writer an export uses, wrapped around a copy of this interpreter, ad-hoc
// signed, and then re-executed with the command line unchanged so the run
// continues inside it. Nothing is impersonated — the identifier belongs to
// Blitsen's development namespace, distinct from the exported application's, and
// `--bundle-id` lets a developer name their own instead.
async function runInsideDevelopmentBundle(options, output) {
  if (process.platform !== "darwin") {
    throw new Error("--dev-bundle is a macOS option: macOS is the only desktop platform that "
      + "ties notification permission and delivery to a bundle identifier, and the other two "
      + "hosts submit notifications from an ordinary executable");
  }
  // Only the name is taken from the configuration here, and the build command it
  // may also carry is deliberately not run: the child reads the same file and
  // owns every other step.
  const { config } = await loadConfig();
  const name = options.name ?? config?.name ?? options.title;
  const { bundle, executable, identifier, rebuilt } = await developmentBundle({
    directory: join(runtimeCacheDir(), "development"),
    name,
    identifier: options.bundleId ?? developmentIdentifier(name),
    launcher: process.execPath,
    version: await packageVersion(),
    ...options.sign === undefined ? {} : { sign: options.sign },
  });
  output.log(`${rebuilt ? "Built" : "Reused"} development bundle ${bundle} (${identifier}). `
    + "That identity is the development host's, not the application you export: macOS records "
    + "notification permission per identifier, so the two are granted and revoked separately.");
  // The same argument vector, under the bundle's own copy of this interpreter.
  // `process.argv` rather than the parsed options, because what has to be
  // reproduced is the command line the developer typed.
  const child = spawn(executable, process.argv.slice(1), {
    stdio: "inherit",
    env: { ...process.env, [DEV_BUNDLE]: bundle },
  });
  return await new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("exit", code => resolve(code ?? 1));
  });
}

export async function main(args, output = console, runtime = null) {
  try {
    let active = runtime;
    const options = parseArgs(args);
    if (options.help) {
      output.log(HELP);
      return 0;
    }
    if (options.version) {
      output.log(await packageVersion());
      return 0;
    }
    // Before the configuration is read, so the configured build command runs
    // once — in the child, which is the process that goes on to open a window.
    if (options.devBundle && !process.env[DEV_BUNDLE]) {
      return await runInsideDevelopmentBundle(options, output);
    }
    // An Android build resolves no runtime at all, and that is the shape of the
    // whole decision (#148): there is no `@blitsen/android-*` package to fetch
    // and nothing to append an application to. It cross-compiles instead, and
    // what has to exist before the user's build command runs is the NDK rather
    // than an addon — checked in the same place and for the same reason.
    if (options.command === "build" && !options.android) {
      // Checked before anything runs: the user's build command must not be spent
      // on an export that cannot link. For a cross-target build that includes
      // fetching the target's runtime, so a target that cannot be built for
      // fails before the build command rather than after it.
      active ??= await targetRuntime(options.target, output);
      if (!active?.build) {
        throw new Error("native build runtime is unavailable; reinstall blitsen for this platform");
      }
    }
    // One answer to "which directory", whether the application is about to be
    // run or exported: the same config, the same build command, the same output.
    // A run that found its application differently from the build beside it is a
    // run that proves nothing about what ships.
    if (options.directory === null) await applyConfiguration(options, output);
    if (options.android && (options.window || options.tray || options.menu)) {
      throw new Error("window, tray and menu configuration is only available to desktop builds");
    }
    const application = await resolveApplication(options.directory);
    // Proxy mode is a way to *run* an application, and neither of the other two
    // commands has anything to read: `doctor` grades files on disk and `build`
    // ingests them, and a dev server serves what it transforms on request
    // rather than a directory either could walk (#67).
    if (application.served && options.command !== "run") {
      throw new Error(`${options.command} needs a directory of built output, not a URL: `
        + `a dev server has no output directory to ${options.command === "doctor" ? "scan" : "ingest"}. `
        + "Run your build and point it at the output.");
    }
    if (options.command === "doctor") {
      const report = await doctorApplication(application.root, { target: options.target });
      if (options.json) output.log(JSON.stringify(report, null, 2));
      else {
        for (const diagnostic of report.diagnostics) {
          const writer = diagnostic.severity === "error" ? output.error : output.log;
          writer.call(output, formatDiagnostic(diagnostic));
        }
        output.log(`Doctor scanned ${report.files} files: ${report.errors} errors, ${report.warnings} warnings.`);
      }
      return report.errors === 0 ? 0 : 1;
    }
    if (options.command === "build") {
      reportStep(output, { step: "ingest", detail: application.entrypoint });
      // Which `native:` modules exist is a property of the platform and never of
      // the architecture — `native-modules.mjs` says so and the table has no
      // arch axis — so an APK carrying several ABIs is graded once, against the
      // Android row #147 landed. Grading per ABI would print the same findings
      // twice under different names.
      const report = await doctorApplication(application.root,
        { target: options.android ? "android-arm64" : options.target });
      reportStep(output, {
        step: "scan",
        detail: `${report.files} files, ${report.errors} errors, ${report.warnings} warnings`,
        notes: report.diagnostics.filter(item => item.severity !== "error").map(formatDiagnostic),
      });
      // Errors go to stderr so a CI log shows the blocking file without the noise.
      for (const diagnostic of report.diagnostics.filter(item => item.severity === "error")) {
        output.error(`${NOTE_INDENT}${formatDiagnostic(diagnostic)}`);
      }
      if (report.errors > 0 && !options.acceptErrors) {
        throw new Error(`${report.errors} compatibility `
          + `${report.errors === 1 ? "error blocks" : "errors block"} this build; `
          + "run 'blitsen doctor' for the full report, "
          + "or --accept-errors to export anyway with the reported behaviour missing");
      }
      if (options.android) return await buildAndroidArtifact(options, application, output);
      // Steps ③–⑤ report themselves as they run: only the exporter knows when
      // each one finished, and a long link should not look like a hang.
      const result = await active.build({
        ...application,
        ...options,
        // The scan already answered "does this application open a raw HID
        // device", so packaging reads it from the report rather than asking for
        // a configuration key that would only repeat what the imports say.
        hid: report.diagnostics.some(item => item.code === "NATIVE_HID_ACCESS"),
        outfile: buildOutfile(options),
        progress: event => reportStep(output, event),
        onNotice: message => output.error(message),
      });
      output.log(`Built ${result.outfile} (${result.assets} assets, ${result.bytes} bytes)`);
      // Issue #73: the export records the runtime it linked, so the line that
      // announces the artifact names it too.
      if (result.runtime) output.log(`Runtime: ${describeRuntime(result.runtime)}`);
      if (result.assetDirectory) output.log(`Side-loaded assets: ${result.assetDirectory}`);
      // Issue #121: the line comes off only for an export that carries the
      // notices it owes, and it is the artifact that carries them — `<the
      // executable> --licenses` prints what was embedded. An export without
      // them still says so, because that is still true of it: a Phase 1 export
      // carries a copy of Bun, whose own LGPL flow is not automated here
      // (docs/LICENSING.md).
      if (result.notices) {
        output.log(`Third-party notices: embedded, ${result.notices.bytes} bytes `
          + "(run the executable with --licenses)");
      } else {
        output.log("This export is not cleared for redistribution: it carries no third-party "
          + "notices (docs/LICENSING.md).");
      }
      return 0;
    }
    active ??= await hostRuntime(output);
    if (!active?.openDirectory) {
      throw new Error("native addon is unavailable; reinstall blitsen for this platform");
    }
    if (application.served) {
      output.log(`Serving from ${application.root} — your dev server owns the files, `
        + "reloading and source maps; this window is the tab.");
    }
    await active.openDirectory({ ...application, ...options });
    // Nothing local to watch when the files are served: the dev server is
    // already watching them, and its own channel is what tells the document.
    const watcher = !application.served && active.reloadCSS && active.reloadDirectory
      ? watchApplication(application.root, active, output)
      : null;
    try {
      if (active.pumpWindow) {
        const pacing = { nextFrame: performance.now() };
        while (active.pumpWindow()) {
          const delay = frameDelay(pacing, performance.now());
          await (active.waitForNextFrame?.(delay)
            ?? new Promise(resolve => setTimeout(resolve, delay)));
        }
      }
    } finally {
      watcher?.close();
    }
    return 0;
  } catch (error) {
    output.error(`blitsen: ${error.message}`);
    return 1;
  }
}

import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { access, copyFile, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { basename, dirname, extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { gzipSync } from "node:zlib";
import {
  planIngest, rewriteRootRelativeReferences,
} from "./application-ingest.mjs";
import { describeExecutableBinary, describeNativeBinary, readContainerHeader } from "./binary.mjs";
import { linkBundle } from "./bundle.mjs";
import { REWRITTEN_EXTENSIONS } from "./files.mjs";
import { frameDelay } from "./frame-pacing.mjs";
import { activationEntryPoint, packageBuild, pngDimensions, signArtifact } from "./packaging.mjs";
import { describeRuntime, hostTarget, requestedHost, resolvePhase2Runtime } from "./runtime.mjs";

export { describeExecutableBinary, describeNativeBinary } from "./binary.mjs";
export {
  globMatcher, planIngest, rewriteRootRelativeReferences,
} from "./application-ingest.mjs";

// Blitsen names a target the way Node does — `process.platform`-`process.arch` —
// and Bun names its own compile targets differently. This is the whole of the
// translation, and it is a closed set: `TARGETS` and this map must stay the same
// six, which `cli-runtime.test.mjs` checks.
const BUN_TARGETS = {
  "darwin-arm64": "bun-darwin-arm64", "darwin-x64": "bun-darwin-x64",
  "linux-arm64": "bun-linux-arm64", "linux-x64": "bun-linux-x64",
  "win32-arm64": "bun-windows-arm64", "win32-x64": "bun-windows-x64",
};

const ADDON_EXTENSION = ".node";
const TRAY_BUNDLE_ICON = "blitsen.tray.png";
const trayMenuBundleIcon = index => `blitsen.tray-menu.${index}.png`;
const NAPI_ENTRYPOINT = "napi_register_module_v1";
// Every other asset in an export is portable bytes; a .node is a host shared
// library, and is the one thing that can be architecturally wrong. Checked
// against the target being built for rather than the host, because those differ
// under `--target` (#72) — and checked here rather than discovered at dlopen in
// front of a user.
async function inspectAddon(staged, path, target) {
  const bytes = await readFile(staged);
  const binary = describeNativeBinary(bytes);
  if (!binary) {
    throw new Error(`${path} is not a native addon: a .node file must be an ELF, Mach-O or PE `
      + "shared library");
  }
  const [platform, architecture] = [target.slice(0, target.lastIndexOf("-")),
    target.slice(target.lastIndexOf("-") + 1)];
  if (binary.platform !== platform || !binary.architectures.includes(architecture)) {
    throw new Error(`native addon ${path} is built for `
      + `${binary.platform}-${binary.architectures.join("/")} (${binary.format}), `
      + `but this export runs on ${target}`
      + (target === hostTarget() ? "" : " (--target)"));
  }
  // Bun loads Node-API addons only. A V8/NAN addon is a valid shared library for
  // this host and would pass every check above, then fail at require.
  if (!bytes.includes(NAPI_ENTRYPOINT)) {
    throw new Error(`native addon ${path} does not export ${NAPI_ENTRYPOINT}: `
      + "Blitsen loads Node-API addons, not V8/NAN addons");
  }
}

// An addon normally lives outside the directory being ingested —
// node_modules/<package>/build/Release/*.node, target/release/*.so — where
// neither the reachability walk nor --include can name it, since both are bounded
// by that directory. Declaring it is therefore a separate act from keeping a file.
async function planAddons(root, addons) {
  const planned = new Map();
  for (const declared of addons) {
    const source = resolve(declared);
    if (extname(source).toLowerCase() !== ADDON_EXTENSION) {
      throw new Error(`a native addon must be a ${ADDON_EXTENSION} file: ${declared} `
        + "(rename the shared library, which is what require resolves)");
    }
    if (!(await stat(source).catch(() => null))?.isFile()) {
      throw new Error(`native addon does not exist: ${declared}`);
    }
    // An addon already inside the output keeps its place, so the specifier the
    // application was written against still resolves; one from outside lands at
    // the top of the application tree under its own name.
    const inside = relative(root, source);
    const path = inside && !inside.startsWith("..") && !isAbsolute(inside)
      ? inside.split(sep).join("/")
      : basename(source);
    const existing = planned.get(path);
    if (existing !== undefined && existing !== source) {
      throw new Error(`two native addons would both be exported as ${path}: ${existing} and ${source}`);
    }
    planned.set(path, source);
  }
  return planned;
}

async function hashFile(absolute) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(absolute)) hash.update(chunk);
  return hash.digest("hex");
}

// The runtime is either the descriptor the resolver produced (src/runtime.mjs) or a
// bare addon path, which is what a repository script and bin/blitsen.mjs hand over.
// Either way the export records what it linked against — issue #73.
export function runtimeRecord(runtime) {
  const record = typeof runtime === "string" ? { path: runtime } : runtime ?? {};
  if (!record.path) {
    throw new Error("native addon is unavailable; reinstall blitsen for this platform");
  }
  return {
    path: record.path,
    target: record.target ?? hostTarget(),
    version: record.version ?? null,
    package: record.package ?? null,
    source: record.source ?? "path",
  };
}

/**
 * The activation envelope a platform entry point started this process with (#252).
 *
 * One option, read the same way by both hosts: the Phase 2 runtime parses it in
 * `blitsen-runtime`, and the generated Phase 1 launcher inlines this function so
 * an export that links Bun does not read its command line differently from one
 * that does not. `null` is the ordinary launch, which carries no envelope.
 */
export function notificationActivation(argv) {
  const index = argv.indexOf("--notification-activation");
  return index >= 0 && index + 1 < argv.length ? argv[index + 1] : null;
}

export function launcherSource(assets, options) {
  const embedded = options.layout === "embedded";
  const imports = embedded
    ? assets.map((asset, index) =>
      `import asset${index} from ${JSON.stringify(`./app/${asset.path}`)} with { type: "file" };`).join("\n")
    : "";
  const manifest = assets.map((asset, index) =>
    `{ path: ${JSON.stringify(asset.path)}, hash: ${JSON.stringify(asset.hash)}`
    + `${embedded ? `, source: asset${index}` : ""} }`).join(",\n  ");
  const prelude = embedded
    ? `const root = await mkdtemp(join(tmpdir(), "blitsen-app-"));
const cleanup = () => rm(root, { recursive: true, force: true });
for (const asset of assets) {
  const destination = join(root, ...asset.path.split("/"));
  await mkdir(dirname(destination), { recursive: true });
  await writeFile(destination, await Bun.file(asset.source).arrayBuffer());
}`
    : `const root = join(dirname(process.execPath), ${JSON.stringify(options.assetDirectory)});
const cleanup = () => {};
for (const asset of assets) {
  if (!await Bun.file(join(root, ...asset.path.split("/"))).exists())
    throw new Error("missing side-loaded asset: " + asset.path + " (expected under " + root + ")");
}`;
  const { path: _path, ...stamp } = options.runtime;
  const windowOptions = JSON.stringify(options.window ?? null);
  const trayOptions = JSON.stringify(options.tray ?? null);
  const menuOptions = JSON.stringify(options.menu ?? null);
  const activationOptions = JSON.stringify(options.activation ?? null);
  return `import addonPath from "./blitsen.node" with { type: "file" };
import { createRequire } from "node:module";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
${imports}

${frameDelay.toString()}

${notificationActivation.toString()}

// Issue #73: an export names the runtime it was built against, in the binary and at
// run time. Parsed from one string so the record survives bundling as a contiguous
// literal a shipped artifact can be searched for. The linking path is deliberately
// absent: it is machine-local.
globalThis[Symbol.for("blitsen.runtime")] = JSON.parse(${JSON.stringify(JSON.stringify(stamp))});

const assets = [
  ${manifest}
];
const startupTray = ${trayOptions};
const startupMenu = ${menuOptions};
const startupActivation = ${activationOptions};
const native = createRequire(import.meta.url)(addonPath);
${prelude}
try {
  const entrypoint = join(root, "index.html");
  const engine = new native.Engine();
  if (process.env.BLITSEN_STANDALONE_CHECK === "1") {
    native.runDocumentScriptsHarness(entrypoint, ${options.width}, ${options.height});
    // Turning the loop rather than only sleeping through it: a fetch, an image
    // decode and a timer all land on the animation-frame tick, so a check that
    // slept would report on an application whose asynchronous work had not
    // happened. The Phase 2 check settles the same way, which is what lets the
    // two be compared line for line (issue #90).
    const settle = async () => {
      const deadline = performance.now()
        + Number(process.env.BLITSEN_STANDALONE_CHECK_DELAY || 50);
      do {
        native.evaluateDocumentHarness(
          "globalThis.__blitsenAnimationFrameTick(" + performance.now() + ")");
        await Bun.sleep(4);
      } while (performance.now() < deadline);
    };
    await settle();
    native.snapshotDocumentHarness();
    if (process.env.BLITSEN_STANDALONE_CHECK_SCRIPT)
      native.evaluateDocumentHarness(process.env.BLITSEN_STANDALONE_CHECK_SCRIPT);
    await settle();
    if (process.env.BLITSEN_STANDALONE_CHECK_ASSERT)
      native.evaluateDocumentHarness(process.env.BLITSEN_STANDALONE_CHECK_ASSERT);
    native.snapshotDocumentHarness();
    console.log("Blitsen standalone check passed (${assets.length} ${options.layout} assets)");
    console.log("Blitsen runtime: " + ${JSON.stringify(describeRuntime(options.runtime))});
  } else {
    engine.openDirectory({
      root,
      entrypoint,
      directory: root,
      width: ${options.width},
      height: ${options.height},
      title: ${JSON.stringify(options.title)},
      ...(${windowOptions} === null ? {} : { window: ${windowOptions} }),
      ...(startupTray === null ? {} : {
        tray: {
          icon: join(root, ${JSON.stringify(TRAY_BUNDLE_ICON)}),
          tooltip: startupTray.tooltip,
          openOnClick: startupTray.openOnClick,
          closeToTray: startupTray.closeToTray,
          menuJson: JSON.stringify(startupTray.contextMenu ?? []),
          menuIcons: (startupTray.menuIcons ?? []).map(path => join(root, path)),
        },
      }),
      ...(startupMenu === null ? {} : {
        menu: { menuJson: JSON.stringify(startupMenu.menu ?? []) },
      }),
      activation: {
        ...(startupActivation ?? {}),
        launchedBy: notificationActivation(process.argv),
      },
    });
    const frameLimit = Number(process.env.BLITSEN_STANDALONE_FRAMES || 0);
    const warmupFrames = Number(process.env.BLITSEN_STANDALONE_WARMUP_FRAMES || 0);
    let started = performance.now();
    const pacing = { nextFrame: started };
    let frames = 0;
    while (engine.pumpWindow()) {
      frames += 1;
      if (frames === warmupFrames) started = performance.now();
      if (frameLimit > 0 && frames >= frameLimit + warmupFrames) break;
      await Bun.sleep(frameDelay(pacing, performance.now()));
    }
    if (frameLimit > 0) {
      const cadence = frameLimit * 1000 / Math.max(1, performance.now() - started);
      console.log(\`Blitsen native frame check passed (\${frameLimit} frames at \${cadence.toFixed(1)} fps)\`);
    }
  }
} catch (error) {
  // What the runtime prints for the same failure: one line, named, on stderr.
  // Letting it escape instead gave a Bun stack trace through the generated
  // launcher, which is this file's business rather than the user's — and made
  // the two hosts describe the same refusal differently (issue #90).
  process.stderr.write("blitsen: " + (error?.message ?? String(error)) + "\\n");
  await cleanup();
  process.exit(1);
} finally {
  await cleanup();
}
`;
}

export function defaultOutfile(root) {
  return resolve(process.cwd(), basename(root));
}

function summarize(paths, limit = 5) {
  const shown = paths.slice(0, limit).join(", ");
  return paths.length > limit ? `${shown}, and ${paths.length - limit} more` : shown;
}

/** What the runtime carries the notices as, inside an export's bundle. */
export const NOTICES_BUNDLE_FILE = "blitsen.notices.txt.gz";

/**
 * The third-party notices shipped beside the runtime being linked (issue #121).
 *
 * Generated where that runtime was built — this checkout, or the release job
 * that produced its platform package — because deriving them needs the
 * dependency graph and a user's machine has no toolchain (P9). `null` means the
 * runtime shipped without them, which is not fatal and is not silent: the build
 * says the export is not cleared for redistribution, which is the truth.
 */
async function embeddedNotices(runtimePath) {
  const path = process.env.BLITSEN_NOTICES_PATH ?? join(dirname(runtimePath), "NOTICES.txt");
  const text = await readFile(path).catch(() => null);
  if (text === null) return null;
  return {
    path,
    bytes: text.length,
    // Deterministic: two builds of the same notices produce the same bytes, so
    // an export stays reproducible (#71).
    gzip: gzipSync(text, { level: 9, mtime: 0 }),
    file: NOTICES_BUNDLE_FILE,
  };
}

async function prepareStandaloneBuild({ root, outfile, force, assets, target, platform }, runtime) {
  const linkedRuntime = runtimeRecord(runtime);
  // The target being built for, and the platform that follows from it. Taken
  // from the runtime that was actually linked when no `--target` was given, so
  // the executable and the addon inside it can never disagree.
  const buildTarget = target ?? linkedRuntime.target ?? hostTarget();
  const buildPlatform = platform ?? buildTarget.slice(0, buildTarget.lastIndexOf("-"));
  if (linkedRuntime.target !== buildTarget) {
    throw new Error(`the linked runtime is for ${linkedRuntime.target}, `
      + `but this build targets ${buildTarget}`);
  }
  const nativePath = linkedRuntime.path;
  // Which host this export links into is not a flag and not a config key: the
  // npm surface is identical across the swap (TECH.md §16.7). Unset — the
  // ordinary case — the exporter decides once it knows what the application
  // carries, below; BLITSEN_HOST overrides that decision either way.
  const requested = requestedHost();
  if (!["embedded", "side-loaded"].includes(assets)) {
    throw new Error(`unknown asset layout: ${assets} (expected embedded or side-loaded)`);
  }
  await access(nativePath).catch(() => {
    throw new Error(`native addon is unavailable: ${nativePath}`);
  });

  // The runtime is the one thing in the export that must match the target and
  // cannot be checked once it is inside the executable. It matters most under
  // `--target`, where BLITSEN_NATIVE_PATH or a stale cache entry could otherwise
  // put this host's addon inside an executable for another platform — which
  // links, ships, and then fails at `dlopen` in front of whoever runs it.
  const runtimeBinary = describeNativeBinary(await readFile(nativePath));
  const targetPlatform = buildTarget.slice(0, buildTarget.lastIndexOf("-"));
  const targetArchitecture = buildTarget.slice(buildTarget.lastIndexOf("-") + 1);
  if (!runtimeBinary) {
    throw new Error(`the linked runtime is not a shared library: ${nativePath}`);
  }
  if (runtimeBinary.platform !== targetPlatform
    || !runtimeBinary.architectures.includes(targetArchitecture)) {
    throw new Error(`the linked runtime is built for `
      + `${runtimeBinary.platform}-${runtimeBinary.architectures.join("/")} `
      + `(${runtimeBinary.format}), but this build targets ${buildTarget}: ${nativePath}`);
  }

  // Windows executes by extension, so a Windows export is named `.exe` whoever
  // asked for what. `bun build --compile` already appends it on the Phase 1
  // path; the Phase 2 path links the bundle to exactly the name it is given, so
  // an unsuffixed one produced a `win32` artifact that Windows will not run and
  // that nothing could spawn. Decided from the build target rather than the host,
  // because `--target win32-x64` from Linux owes the same name.
  const requestedDestination = resolve(outfile ?? defaultOutfile(root));
  const destination = buildTarget.startsWith("win32-") && extname(requestedDestination) !== ".exe"
    ? `${requestedDestination}.exe`
    : requestedDestination;
  const assetDirectory = `${basename(destination)}.assets`;
  const sideLoaded = join(dirname(destination), assetDirectory);
  if (!force) {
    const occupied = assets === "side-loaded" ? [destination, sideLoaded] : [destination];
    for (const path of occupied) {
      if (await stat(path).catch(() => null)) {
        throw new Error(`output already exists: ${path} (pass --force to replace it)`);
      }
    }
  }
  return {
    linkedRuntime, buildTarget, buildPlatform, nativePath, requested,
    targetPlatform, targetArchitecture, destination, assetDirectory, sideLoaded,
  };
}

async function planApplication(root, include, addons, trayAssets = []) {
  const plan = await planIngest(root, { include });
  const carried = new Map(plan.files.map(file => [file.relative, file.absolute]));
  for (const [path, source] of await planAddons(root, addons)) {
    const occupant = carried.get(path);
    if (occupant !== undefined && occupant !== source) {
      throw new Error(`native addon ${source} would replace ${path} in the application output`);
    }
    carried.set(path, source);
  }
  for (const { path, source, description } of trayAssets) {
    if (!(await stat(source).catch(() => null))?.isFile()) {
      throw new Error(`${description} does not exist: ${source}`);
    }
    pngDimensions(await readFile(source), source);
    if (carried.has(path) && carried.get(path) !== source) {
      throw new Error(`${path} is reserved for ${description}`);
    }
    carried.set(path, source);
  }
  const traySources = new Set(trayAssets.map(asset => resolve(asset.source)));
  return {
    plan,
    carried,
    unreferenced: plan.unreferenced.filter(path => !carried.has(path)
      && !traySources.has(resolve(root, ...path.split("/")))),
  };
}

async function stageApplication({ plan, carried, staging, buildTarget }) {
  const manifest = [];
  for (const [path, absolute] of [...carried].sort(([left], [right]) => left.localeCompare(right))) {
    const staged = join(staging, "app", ...path.split("/"));
    await mkdir(dirname(staged), { recursive: true });
    if (REWRITTEN_EXTENSIONS.includes(extname(path).toLowerCase())) {
      const resolutions = plan.resolutions.get(path);
      const source = rewriteRootRelativeReferences(
        await readFile(absolute, "utf8"),
        path,
        reference => resolutions?.get(reference) ?? null,
      );
      await writeFile(staged, source);
    } else {
      await copyFile(absolute, staged);
    }
    // Checked however it arrived: declared, reached from a script, or kept by
    // --include. A carried addon that cannot load is worse than an absent one.
    const native = extname(path).toLowerCase() === ADDON_EXTENSION;
    if (native) await inspectAddon(staged, path, buildTarget);
    manifest.push({ path, hash: await hashFile(staged), ...native ? { native: true } : {} });
  }
  return {
    manifest,
    carriedAddons: manifest.filter(asset => asset.native).map(asset => asset.path),
  };
}

function selectStandaloneHost(requested, carriedAddons) {
  const host = requested ?? (carriedAddons.length > 0 ? "bun" : "blitsen");
  if (host === "blitsen" && carriedAddons.length > 0) {
    throw new Error(`BLITSEN_HOST=blitsen cannot load a carried native addon `
      + `(${summarize(carriedAddons)}): the Phase 2 host has no Node-API. `
      + "Drop the addon, or leave BLITSEN_HOST unset and the export links the host that can.");
  }
  return host;
}

function reportCollection(progress, { manifest, assets, unreferenced, carriedAddons }) {
  progress({
    step: "collect",
    detail: `${manifest.length} ${assets} assets`,
    notes: [
      ...unreferenced.length === 0 ? [] : [
        `dropped ${unreferenced.length} files unreachable from index.html `
        + `(--include <glob> keeps them): ${summarize(unreferenced)}`,
      ],
      ...carriedAddons.length === 0 ? [] : [
        `carried ${carriedAddons.length} native `
        + `${carriedAddons.length === 1 ? "addon" : "addons"}: ${summarize(carriedAddons)} `
        + "(load one from a module script with createRequire(import.meta.url))",
        "linked the Bun host, which is the one that can load a Node-API addon: "
        + "this export carries a copy of Bun and is roughly 95 MB larger than one without",
      ],
    ],
  });
}

async function linkPhase2({
  buildTarget, targetPlatform, targetArchitecture, onNotice, manifest, staging,
  linkedRuntime, width, height, title, window, tray, menu, activation, assets, destination,
}) {
  // After the checks above, deliberately: `fetch` is on the same terms as the
  // addon's own resolution (#72) — a build for this host never reaches the
  // network, and a cross-target one has no other way to obtain that target's
  // runtime — and a build that is already going to be refused should be
  // refused before it downloads anything.
  const phase2Runtime = await resolvePhase2Runtime({
    target: buildTarget, fetch: buildTarget !== hostTarget(),
    ...(onNotice ? { onNotice } : {}),
  });
  // The same check the addon gets above, on the artifact the export is
  // literally made of: a Phase 2 export is this executable with the
  // application appended, so a runtime for the wrong platform is not a dlopen
  // failure later — it is the whole product, built for the wrong machine and
  // named as though it were not. `BLITSEN_RUNTIME_PATH` reaches here ahead of
  // everything, including under `--target`.
  const linked = describeExecutableBinary(await readContainerHeader(phase2Runtime.path));
  if (!linked) {
    throw new Error("the linked Phase 2 runtime is not an executable for any supported "
      + `platform: ${phase2Runtime.path}`);
  }
  if (linked.platform !== targetPlatform
    || !linked.architectures.includes(targetArchitecture)) {
    throw new Error("the linked Phase 2 runtime is built for "
      + `${linked.platform}-${linked.architectures.join("/")} (${linked.format}), `
      + `but this build targets ${buildTarget}: ${phase2Runtime.path}`
      + (phase2Runtime.source === "environment"
        ? " — BLITSEN_RUNTIME_PATH names it, and it outranks the target's own runtime"
        : ""));
  }

  // Phase 2 step ④: the application is appended to Blitsen's own runtime as a
  // binary section (TECH.md §10, issue #88). No launcher and no Bun.
  const files = new Map();
  for (const entry of manifest) {
    files.set(entry.path, await readFile(join(staging, "app", ...entry.path.split("/"))));
  }
  // Issue #73: an export names the runtime it was built against, in the binary
  // and at run time — the same record the Phase 1 launcher carries, minus the
  // linking path, which is machine-local.
  const { path: _linkedPath, ...stamp } = linkedRuntime;
  files.set("blitsen.runtime.json", Buffer.from(
    `${JSON.stringify({ width, height, title, window, tray, menu, activation,
      layout: assets, runtime: stamp })}\n`));
  // Issue #121: the notices the artifact owes travel inside it. They are copied
  // from the runtime package because a user's machine has no toolchain to
  // derive them.
  const notices = await embeddedNotices(phase2Runtime.path);
  if (notices !== null) files.set(NOTICES_BUNDLE_FILE, notices.gzip);
  await linkBundle({ runtime: phase2Runtime.path, output: destination, files });
  return notices;
}

async function linkPhase1({
  nativePath, staging, manifest, width, height, title, window, tray, menu, activation, assets,
  assetDirectory, linkedRuntime, destination, buildTarget,
}) {
  await copyFile(nativePath, join(staging, "blitsen.node"));
  const launcher = join(staging, "launcher.mjs");
  await writeFile(launcher, launcherSource(manifest, {
    width, height, title, window, tray, menu, activation, layout: assets, assetDirectory,
    runtime: linkedRuntime,
  }));
  // The Bun host is the one thing here that only Bun can build: `Bun.build`
  // links the launcher into that target's Bun. The CLI otherwise runs anywhere
  // Node does, and an export that needs this path says which half is missing.
  if (globalThis.Bun === undefined) {
    throw new Error("this export links the Bun host, which only Bun can build: "
      + "run the same command with `bun` on PATH, or remove the .node addon that "
      + "asked for it — an application without one links Blitsen's own runtime, "
      + "which needs nothing but this package");
  }
  const result = await Bun.build({
    entrypoints: [launcher],
    // Bun downloads the target's own runtime to compile against, which is what
    // makes a cross-target export possible at all.
    compile: { outfile: destination, target: BUN_TARGETS[buildTarget] },
  });
  if (!result.success) {
    const detail = result.logs.map(log => String(log)).join("\n");
    throw new Error(`standalone compilation failed${detail ? `:\n${detail}` : ""}`);
  }
}

async function writeSideLoadedAssets({ assets, sideLoaded, manifest, staging }) {
  if (assets !== "side-loaded") return;
  await rm(sideLoaded, { recursive: true, force: true });
  for (const entry of manifest) {
    const target = join(sideLoaded, ...entry.path.split("/"));
    await mkdir(dirname(target), { recursive: true });
    await copyFile(join(staging, "app", ...entry.path.split("/")), target);
  }
}

async function finishStandaloneBuild({
  destination, progress, icon, bundleId, appVersion, buildPlatform, title,
  assets, sideLoaded, force, sign, hid,
}) {
  // bun build --compile appends .exe on Windows when the requested path has no
  // extension, so the linked artifact is not always the requested path.
  const linked = await stat(destination).catch(() => null) ? destination : `${destination}.exe`;
  progress({ step: "link", detail: linked });
  // `hid` joins the three flags that ask for platform artifacts: an application
  // that opens a raw HID device needs the udev rule or the entitlement whether
  // or not it also asked for an icon (#247).
  const packaged = icon || bundleId || appVersion || hid
    ? await packageBuild({
      platform: buildPlatform,
      executable: linked,
      title,
      icon,
      identifier: bundleId,
      version: appVersion,
      assetDirectory: assets === "side-loaded" ? sideLoaded : null,
      force,
      hid,
    })
    : null;
  const executable = packaged?.executable ?? linked;
  // The signing hook runs last, over the bundle on macOS and the executable
  // elsewhere, so it sees exactly what ships.
  const signed = sign
    ? await signArtifact({ command: sign, artifact: packaged?.bundle ?? executable })
    : null;
  progress({
    step: "package",
    detail: packaged
      ? `${packaged.platform}: ${packaged.artifacts.join(", ")}`
      : "no platform artifacts requested (--icon, --bundle-id, --app-version)",
    notes: [
      ...packaged?.notes ?? [],
      ...(signed ? [`signed ${signed.artifact} with: ${signed.command}`] : []),
    ],
  });
  return { executable, packaged, signed };
}

export async function buildStandalone(
  {
    root, width, height, title, outfile, force = false, include = [], addons = [],
    assets = "embedded", icon = null, bundleId = null, appVersion = null, sign = null,
    target = null, platform, window = null, tray = null, menu = null, hid = false,
    progress = () => {}, onNotice,
  },
  runtime,
) {
  const prepared = await prepareStandaloneBuild(
    { root, outfile, force, assets, target, platform }, runtime);
  const {
    linkedRuntime, buildTarget, buildPlatform, nativePath, requested,
    targetPlatform, targetArchitecture, destination, assetDirectory, sideLoaded,
  } = prepared;
  const trayAssets = tray ? [
    { path: TRAY_BUNDLE_ICON, source: tray.icon, description: "the configured tray icon" },
    ...(tray.menuIcons ?? []).map((source, index) => ({
      path: trayMenuBundleIcon(index), source, description: `configured tray menu icon ${index + 1}`,
    })),
  ] : [];
  const runtimeTray = tray ? {
    ...tray,
    icon: TRAY_BUNDLE_ICON,
    menuIcons: (tray.menuIcons ?? []).map((_, index) => trayMenuBundleIcon(index)),
  } : null;
  // Recorded before the artifact is linked, because the runtime configuration is
  // written into it: the identity a notification activation is addressed to has
  // to be inside the executable the platform will start (#252).
  const activation = activationEntryPoint(
    { platform: buildPlatform, identifier: bundleId, executable: destination });
  const { plan, carried, unreferenced } = await planApplication(root, include, addons, trayAssets);
  // Bun records the compiled entrypoint's path in the executable, so staging has
  // to be a stable location rather than a temporary one for reproducible output.
  const staging = join(dirname(destination), `.${basename(destination)}.blitsen-build`);
  await rm(staging, { recursive: true, force: true });
  try {
    const { manifest, carriedAddons } = await stageApplication(
      { plan, carried, staging, buildTarget });
    // The host, now that the application is known. Small by default, and one
    // thing overrides that — a capability rather than a preference: a `.node`
    // addon is Node-API, and `createRequire` is Bun's, so the Phase 2 host has
    // no way to load one (TECH.md §12). That export links Phase 1 and pays a
    // copy of Bun for it, because a smaller executable that cannot run the
    // application is not smaller.
    //
    // Module scripts used to be a second override, back when the Phase 2 host
    // loaded JavaScriptCore at run time and the library it found might have no
    // module entry point. The shipped runtime links QuickJS-ng statically and
    // its module loader is stock, so there is no longer a build whose engine
    // could turn up without one.
    const host = selectStandaloneHost(requested, carriedAddons);
    reportCollection(progress, { manifest, assets, unreferenced, carriedAddons });
    let notices = null;
    if (host === "blitsen") {
      notices = await linkPhase2({
        buildTarget, targetPlatform, targetArchitecture, onNotice, manifest, staging,
        linkedRuntime, width, height, title, window, tray: runtimeTray, menu, activation,
        assets, destination,
      });
    } else {
      await linkPhase1({
        nativePath, staging, manifest, width, height, title, window, tray: runtimeTray, menu,
        activation, assets, assetDirectory,
        linkedRuntime, destination, buildTarget,
      });
    }
    await writeSideLoadedAssets({ assets, sideLoaded, manifest, staging });
    const { executable, packaged, signed } = await finishStandaloneBuild({
      destination, progress, icon, bundleId, appVersion, buildPlatform, title,
      assets, sideLoaded, force, sign, hid,
    });
    return {
      outfile: executable,
      host,
      runtime: linkedRuntime,
      layout: assets,
      assets: manifest.length,
      notices,
      manifest,
      addons: carriedAddons,
      unreferenced,
      assetDirectory: assets === "side-loaded" ? packaged?.assetDirectory ?? sideLoaded : null,
      bytes: (await stat(executable)).size,
      packaging: packaged,
      signed,
    };
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

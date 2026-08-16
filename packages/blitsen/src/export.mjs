import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { access, copyFile, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { basename, dirname, extname, isAbsolute, join, posix, relative, resolve, sep } from "node:path";
import { gzipSync } from "node:zlib";
import { describeExecutableBinary, describeNativeBinary, readContainerHeader } from "./binary.mjs";
import { linkBundle } from "./bundle.mjs";
import {
  HTML_EXTENSIONS, REWRITTEN_EXTENSIONS, SCRIPT_EXTENSIONS, walkFiles,
} from "./files.mjs";
import { frameDelay } from "./frame-pacing.mjs";
import { packageBuild, signArtifact } from "./packaging.mjs";
import { describeRuntime, hostTarget, requestedHost, resolvePhase2Runtime } from "./runtime.mjs";

export { describeExecutableBinary, describeNativeBinary } from "./binary.mjs";

// Blitsen names a target the way Node does — `process.platform`-`process.arch` —
// and Bun names its own compile targets differently. This is the whole of the
// translation, and it is a closed set: `TARGETS` and this map must stay the same
// six, which `cli-runtime.test.mjs` checks.
const BUN_TARGETS = {
  "darwin-arm64": "bun-darwin-arm64", "darwin-x64": "bun-darwin-x64",
  "linux-arm64": "bun-linux-arm64", "linux-x64": "bun-linux-x64",
  "win32-arm64": "bun-windows-arm64", "win32-x64": "bun-windows-x64",
};

const HTML_REFERENCES = [
  /<(?:script|img|source|audio|video|track|embed|input)\b[^>]*?\bsrc\s*=\s*["']([^"']*)["']/gi,
  /<link\b[^>]*?\bhref\s*=\s*["']([^"']*)["']/gi,
  /<video\b[^>]*?\bposter\s*=\s*["']([^"']*)["']/gi,
  /<object\b[^>]*?\bdata\s*=\s*["']([^"']*)["']/gi,
];
const CSS_REFERENCES = [
  /url\(\s*["']?([^"')]*)["']?\s*\)/gi,
  /@import\s+["']([^"']*)["']/gi,
];
// Statically analysable module-graph edges only. Computed specifiers are not
// followed; --include is the escape hatch for those.
const SCRIPT_REFERENCES = [
  /\bfrom\s*["']([^"']*)["']/g,
  /\bimport\s*\(?\s*["']([^"']*)["']/g,
  /\bnew\s+URL\s*\(\s*["']([^"']*)["']\s*,\s*import\.meta\.url\s*\)/g,
];
// A bundler resolves `import hero from "./hero.png"` into a bare string literal,
// so no import edge survives for the walk to follow. Every quoted string is
// therefore offered as a candidate, and only kept when it resolves to a file the
// bundler actually emitted — which is why this cannot invent a reference to
// something that is not there. The asymmetry justifies the guess: a false
// positive costs bytes in the export, a false negative costs a missing asset.
const SCRIPT_LITERALS = [
  /"([^"\n\\]*)"/g,
  /'([^'\n\\]*)'/g,
  /`([^`\n\\$]*)`/g,
];

function referencePatterns(file) {
  const extension = extname(file).toLowerCase();
  if (HTML_EXTENSIONS.includes(extension)) return HTML_REFERENCES;
  if (extension === ".css") return CSS_REFERENCES;
  if (SCRIPT_EXTENSIONS.includes(extension)) return [...SCRIPT_REFERENCES, ...SCRIPT_LITERALS];
  return null;
}

function localReference(url) {
  const trimmed = url.trim();
  if (!trimmed || trimmed.startsWith("#") || trimmed.startsWith("//")) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(trimmed)) return null;
  const split = trimmed.search(/[?#]/);
  const pathname = split < 0 ? trimmed : trimmed.slice(0, split);
  return pathname ? { pathname, suffix: split < 0 ? "" : trimmed.slice(split) } : null;
}

// A configured bundler base ("/app/") prefixes server-root URLs with directories
// that do not exist in the output, so drop leading segments until one resolves.
function resolveReference(pathname, sourceFile, exists) {
  if (pathname.startsWith("/")) {
    const segments = pathname.split("/").filter(Boolean);
    for (let index = 0; index < segments.length; index += 1) {
      const candidate = segments.slice(index).join("/");
      if (exists(candidate)) return candidate;
    }
    return null;
  }
  const target = posix.normalize(posix.join(posix.dirname(sourceFile), pathname));
  return target.startsWith("..") || !exists(target) ? null : target;
}

function relativeAssetUrl(target, sourceFile) {
  let rewritten = posix.relative(posix.dirname(sourceFile), target);
  if (!rewritten) rewritten = ".";
  if (!rewritten.startsWith(".")) rewritten = `./${rewritten}`;
  return rewritten;
}

export function rewriteRootRelativeReferences(source, sourceFile, resolveTarget = path => path.slice(1)) {
  const rewrite = (match, prefix, url, suffix = "") => {
    const reference = localReference(url);
    const target = reference && resolveTarget(reference.pathname);
    if (!target) return match;
    return `${prefix}${relativeAssetUrl(target, sourceFile)}${reference.suffix}${suffix}`;
  };
  if (HTML_EXTENSIONS.includes(extname(sourceFile).toLowerCase())) {
    const patterns = [
      /(<(?:script|img|source|audio|video|track|embed|input)\b[^>]*\bsrc\s*=\s*["'])(\/(?!\/)[^"']*)(["'])/gi,
      /(<link\b[^>]*\bhref\s*=\s*["'])(\/(?!\/)[^"']*)(["'])/gi,
      /(<video\b[^>]*\bposter\s*=\s*["'])(\/(?!\/)[^"']*)(["'])/gi,
      /(<object\b[^>]*\bdata\s*=\s*["'])(\/(?!\/)[^"']*)(["'])/gi,
    ];
    return patterns.reduce((rewritten, pattern) => rewritten.replace(pattern, rewrite), source);
  }
  if (extname(sourceFile).toLowerCase() === ".css") {
    return source.replace(/(url\(\s*["']?)(\/(?!\/)[^"')]*)(["']?\s*\))/gi, rewrite);
  }
  return source;
}

export function globMatcher(pattern) {
  const expression = pattern.split("**")
    .map(part => part
      .replace(/[.+^${}()|[\]\\]/g, "\\$&")
      .replace(/\*/g, "[^/]*")
      .replace(/\?/g, "[^/]"))
    .join(".*");
  return new RegExp(`^${expression}$`);
}

// Reachability from the HTML entrypoint. Unreferenced output is pure export size,
// so it is reported and dropped unless an --include glob asks for it.
export async function planIngest(root, { entrypoint = "index.html", include = [] } = {}) {
  const files = await walkFiles(root, {
    filter: (_file, entry) => entry.isFile(),
    onSymlink: file => {
      throw new Error(`application output contains a symbolic link: ${relative(root, file.absolute)}`);
    },
  });
  const byPath = new Map(files.map(file => [file.relative, file]));
  if (!byPath.has(entrypoint)) {
    throw new Error(`missing application entrypoint: ${join(root, entrypoint)}`);
  }
  const exists = path => byPath.has(path);
  const resolutions = new Map();
  const reachable = new Set();
  const unresolved = [];
  const queue = [entrypoint];
  while (queue.length > 0) {
    const current = queue.shift();
    if (reachable.has(current)) continue;
    reachable.add(current);
    const patterns = referencePatterns(current);
    if (!patterns) continue;
    // Script scanning is heuristic, so a string that merely looks like a
    // specifier must not fail an otherwise valid build.
    const authoritative = !SCRIPT_EXTENSIONS.includes(extname(current).toLowerCase());
    const source = await readFile(byPath.get(current).absolute, "utf8");
    const rewrites = new Map();
    resolutions.set(current, rewrites);
    for (const pattern of patterns) {
      pattern.lastIndex = 0;
      let match;
      while ((match = pattern.exec(source))) {
        const reference = localReference(match[1]);
        if (!reference) continue;
        // A chunk-manifest path is written relative to the output root rather
        // than to the chunk holding it, so a literal gets both readings.
        const target = resolveReference(reference.pathname, current, exists)
          ?? (authoritative || reference.pathname.startsWith("/") ? null
            : exists(posix.normalize(reference.pathname)) ? posix.normalize(reference.pathname) : null);
        if (target === null) {
          if (authoritative) unresolved.push({ file: current, url: match[1] });
          continue;
        }
        rewrites.set(reference.pathname, target);
        if (!reachable.has(target)) queue.push(target);
      }
    }
  }
  if (unresolved.length > 0) {
    const detail = unresolved.map(item => `${item.file} references ${item.url}`).join("\n  ");
    throw new Error(`unresolved local references in the application output:\n  ${detail}`);
  }
  const matchers = include.map(globMatcher);
  const kept = file => reachable.has(file.relative)
    || matchers.some(matcher => matcher.test(file.relative));
  return {
    files: files.filter(kept),
    resolutions,
    unreferenced: files.filter(file => !kept(file)).map(file => file.relative),
  };
}

const ADDON_EXTENSION = ".node";
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
  return `import addonPath from "./blitsen.node" with { type: "file" };
import { createRequire } from "node:module";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
${imports}

${frameDelay.toString()}

// Issue #73: an export names the runtime it was built against, in the binary and at
// run time. Parsed from one string so the record survives bundling as a contiguous
// literal a shipped artifact can be searched for. The linking path is deliberately
// absent: it is machine-local.
globalThis[Symbol.for("blitsen.runtime")] = JSON.parse(${JSON.stringify(JSON.stringify(stamp))});

const assets = [
  ${manifest}
];
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

export async function buildStandalone(
  {
    root, width, height, title, outfile, force = false, include = [], addons = [],
    assets = "embedded", icon = null, bundleId = null, appVersion = null, sign = null,
    target = null, platform, progress = () => {}, onNotice,
  },
  runtime,
) {
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

  const plan = await planIngest(root, { include });
  const carried = new Map(plan.files.map(file => [file.relative, file.absolute]));
  for (const [path, source] of await planAddons(root, addons)) {
    const occupant = carried.get(path);
    if (occupant !== undefined && occupant !== source) {
      throw new Error(`native addon ${source} would replace ${path} in the application output`);
    }
    carried.set(path, source);
  }
  const unreferenced = plan.unreferenced.filter(path => !carried.has(path));
  // Bun records the compiled entrypoint's path in the executable, so staging has
  // to be a stable location rather than a temporary one for reproducible output.
  const staging = join(dirname(destination), `.${basename(destination)}.blitsen-build`);
  await rm(staging, { recursive: true, force: true });
  try {
    const manifest = [];
    let notices = null;
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
    const carriedAddons = manifest.filter(asset => asset.native).map(asset => asset.path);
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
    const host = requested ?? (carriedAddons.length > 0 ? "bun" : "blitsen");
    if (host === "blitsen" && carriedAddons.length > 0) {
      throw new Error(`BLITSEN_HOST=blitsen cannot load a carried native addon `
        + `(${summarize(carriedAddons)}): the Phase 2 host has no Node-API. `
        + "Drop the addon, or leave BLITSEN_HOST unset and the export links the host that can.");
    }
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
    // After the checks above, deliberately: `fetch` is on the same terms as the
    // addon's own resolution (#72) — a build for this host never reaches the
    // network, and a cross-target one has no other way to obtain that target's
    // runtime — and a build that is already going to be refused should be
    // refused before it downloads anything.
    const phase2Runtime = host === "blitsen"
      ? await resolvePhase2Runtime({
        target: buildTarget, fetch: buildTarget !== hostTarget(),
        ...(onNotice ? { onNotice } : {}),
      })
      : null;
    if (host === "blitsen") {
      // The same check the addon gets above, on the artifact the export is
      // literally made of: a Phase 2 export is this executable with the
      // application appended, so a runtime for the wrong platform is not a
      // dlopen failure later — it is the whole product, built for the wrong
      // machine and named as though it were not. `BLITSEN_RUNTIME_PATH` reaches
      // here ahead of everything, including under `--target`, which is exactly
      // how a release job that sets it for its own tests silently produced an
      // ELF called `App.exe` (#134).
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
      // Phase 2 step ④: the application is appended to Blitsen's own runtime
      // as a binary section (TECH.md §10, issue #88). No launcher and no Bun —
      // the executable reads its own bundle at startup.
      const files = new Map();
      for (const entry of manifest) {
        files.set(entry.path, await readFile(join(staging, "app", ...entry.path.split("/"))));
      }
      // Issue #73: an export names the runtime it was built against, in the
      // binary and at run time — the same record the Phase 1 launcher carries,
      // minus the linking path, which is machine-local. Written compactly so
      // the record survives as a contiguous literal a shipped artifact can be
      // searched for, exactly as it does on the other host.
      const { path: _linkedPath, ...stamp } = linkedRuntime;
      files.set("blitsen.runtime.json", Buffer.from(
        `${JSON.stringify({ width, height, title, layout: assets, runtime: stamp })}\n`));
      // Issue #121: the notices the artifact owes travel inside it. They are
      // generated where the runtime is built and shipped in its platform
      // package, so this is a copy rather than a computation — there is no
      // toolchain on a user's machine to derive them from.
      notices = await embeddedNotices(phase2Runtime.path);
      if (notices !== null) files.set(NOTICES_BUNDLE_FILE, notices.gzip);
      await linkBundle({ runtime: phase2Runtime.path, output: destination, files });
    } else {
      await copyFile(nativePath, join(staging, "blitsen.node"));
      const launcher = join(staging, "launcher.mjs");
      await writeFile(launcher, launcherSource(manifest, {
        width, height, title, layout: assets, assetDirectory, runtime: linkedRuntime,
      }));
      // The Bun host is the one thing here that only Bun can build: `Bun.build`
      // is what links the launcher into that target's Bun. The CLI otherwise
      // runs anywhere Node does, and an export that needs this path says which
      // half is missing rather than dying on an undefined global (#131).
      if (globalThis.Bun === undefined) {
        throw new Error("this export links the Bun host, which only Bun can build: "
          + "run the same command with `bun` on PATH, or remove the .node addon that "
          + "asked for it — an application without one links Blitsen's own runtime, "
          + "which needs nothing but this package");
      }
      const result = await Bun.build({
        entrypoints: [launcher],
        // Bun downloads the target's own runtime to compile against, which is what
        // makes a cross-target export possible at all: the launcher is bundled
        // here and linked into that runtime rather than into this host's.
        compile: { outfile: destination, target: BUN_TARGETS[buildTarget] },
      });
      if (!result.success) {
        const detail = result.logs.map(log => String(log)).join("\n");
        throw new Error(`standalone compilation failed${detail ? `:\n${detail}` : ""}`);
      }
    }
    if (assets === "side-loaded") {
      await rm(sideLoaded, { recursive: true, force: true });
      for (const entry of manifest) {
        const target = join(sideLoaded, ...entry.path.split("/"));
        await mkdir(dirname(target), { recursive: true });
        await copyFile(join(staging, "app", ...entry.path.split("/")), target);
      }
    }
    // bun build --compile appends .exe on Windows when the requested path has no
    // extension, so the linked artifact is not always the requested path.
    const linked = await stat(destination).catch(() => null) ? destination : `${destination}.exe`;
    progress({ step: "link", detail: linked });
    const packaged = icon || bundleId || appVersion
      ? await packageBuild({
        platform: buildPlatform,
        executable: linked,
        title,
        icon,
        identifier: bundleId,
        version: appVersion,
        assetDirectory: assets === "side-loaded" ? sideLoaded : null,
        force,
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

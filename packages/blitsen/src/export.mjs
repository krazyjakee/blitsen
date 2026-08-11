import { access, copyFile, mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { basename, dirname, extname, join, posix, relative, resolve, sep } from "node:path";
import { packageBuild, signArtifact } from "./packaging.mjs";

const HTML_EXTENSIONS = [".html", ".htm"];
const SCRIPT_EXTENSIONS = [".js", ".mjs", ".cjs"];
const REWRITTEN_EXTENSIONS = [...HTML_EXTENSIONS, ".css"];

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

async function collectFiles(root, directory = root) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const absolute = join(directory, entry.name);
    if (entry.isSymbolicLink()) {
      throw new Error(`application output contains a symbolic link: ${relative(root, absolute)}`);
    }
    if (entry.isDirectory()) files.push(...await collectFiles(root, absolute));
    else if (entry.isFile()) files.push({
      absolute,
      relative: relative(root, absolute).split(sep).join("/"),
    });
  }
  return files.sort((left, right) => left.relative.localeCompare(right.relative));
}

function referencePatterns(file) {
  const extension = extname(file).toLowerCase();
  if (HTML_EXTENSIONS.includes(extension)) return HTML_REFERENCES;
  if (extension === ".css") return CSS_REFERENCES;
  if (SCRIPT_EXTENSIONS.includes(extension)) return SCRIPT_REFERENCES;
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
  const files = await collectFiles(root);
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
    const authoritative = patterns !== SCRIPT_REFERENCES;
    const source = await readFile(byPath.get(current).absolute, "utf8");
    const rewrites = new Map();
    resolutions.set(current, rewrites);
    for (const pattern of patterns) {
      pattern.lastIndex = 0;
      let match;
      while ((match = pattern.exec(source))) {
        const reference = localReference(match[1]);
        if (!reference) continue;
        const target = resolveReference(reference.pathname, current, exists);
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

async function hashFile(absolute) {
  const hasher = new Bun.CryptoHasher("sha256");
  for await (const chunk of Bun.file(absolute).stream()) hasher.update(chunk);
  return hasher.digest("hex");
}

function launcherSource(assets, options) {
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
  return `import addonPath from "./blitsen.node" with { type: "file" };
import { createRequire } from "node:module";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
${imports}

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
    await Bun.sleep(Number(process.env.BLITSEN_STANDALONE_CHECK_DELAY || 50));
    native.snapshotDocumentHarness();
    if (process.env.BLITSEN_STANDALONE_CHECK_SCRIPT)
      native.evaluateDocumentHarness(process.env.BLITSEN_STANDALONE_CHECK_SCRIPT);
    await Bun.sleep(Number(process.env.BLITSEN_STANDALONE_CHECK_DELAY || 50));
    if (process.env.BLITSEN_STANDALONE_CHECK_ASSERT)
      native.evaluateDocumentHarness(process.env.BLITSEN_STANDALONE_CHECK_ASSERT);
    native.snapshotDocumentHarness();
    console.log("Blitsen standalone check passed (${assets.length} ${options.layout} assets)");
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
    const frameInterval = 1000 / 60;
    let nextFrame = started;
    let frames = 0;
    while (engine.pumpWindow()) {
      frames += 1;
      if (frames === warmupFrames) started = performance.now();
      if (frameLimit > 0 && frames >= frameLimit + warmupFrames) break;
      nextFrame += frameInterval;
      const now = performance.now();
      if (nextFrame < now - frameInterval) nextFrame = now;
      await Bun.sleep(Math.max(0, nextFrame - now));
    }
    if (frameLimit > 0) {
      const cadence = frameLimit * 1000 / Math.max(1, performance.now() - started);
      console.log(\`Blitsen native frame check passed (\${frameLimit} frames at \${cadence.toFixed(1)} fps)\`);
    }
  }
} finally {
  await cleanup();
}
`;
}

export function defaultOutfile(root) {
  return resolve(process.cwd(), basename(root));
}

export async function buildStandalone(
  {
    root, width, height, title, outfile, force = false, include = [], assets = "embedded",
    icon = null, bundleId = null, appVersion = null, sign = null,
    platform = process.platform,
  },
  nativePath,
) {
  if (!nativePath) throw new Error("native addon is unavailable; reinstall blitsen for this platform");
  if (!["embedded", "side-loaded"].includes(assets)) {
    throw new Error(`unknown asset layout: ${assets} (expected embedded or side-loaded)`);
  }
  await access(nativePath).catch(() => {
    throw new Error(`native addon is unavailable: ${nativePath}`);
  });
  const destination = resolve(outfile ?? defaultOutfile(root));
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
  // Bun records the compiled entrypoint's path in the executable, so staging has
  // to be a stable location rather than a temporary one for reproducible output.
  const staging = join(dirname(destination), `.${basename(destination)}.blitsen-build`);
  await rm(staging, { recursive: true, force: true });
  try {
    const manifest = [];
    for (const file of plan.files) {
      const staged = join(staging, "app", ...file.relative.split("/"));
      await mkdir(dirname(staged), { recursive: true });
      if (REWRITTEN_EXTENSIONS.includes(extname(file.relative).toLowerCase())) {
        const resolutions = plan.resolutions.get(file.relative);
        const source = rewriteRootRelativeReferences(
          await readFile(file.absolute, "utf8"),
          file.relative,
          path => resolutions?.get(path) ?? null,
        );
        await writeFile(staged, source);
      } else {
        await copyFile(file.absolute, staged);
      }
      manifest.push({ path: file.relative, hash: await hashFile(staged) });
    }
    await copyFile(nativePath, join(staging, "blitsen.node"));
    const launcher = join(staging, "launcher.mjs");
    await writeFile(launcher, launcherSource(manifest, {
      width, height, title, layout: assets, assetDirectory,
    }));
    const result = await Bun.build({
      entrypoints: [launcher],
      compile: { outfile: destination },
    });
    if (!result.success) {
      const detail = result.logs.map(log => String(log)).join("\n");
      throw new Error(`standalone compilation failed${detail ? `:\n${detail}` : ""}`);
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
    const packaged = icon || bundleId || appVersion
      ? await packageBuild({
        platform,
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
      ? await signArtifact({ platform, command: sign, artifact: packaged?.bundle ?? executable })
      : null;
    return {
      outfile: executable,
      layout: assets,
      assets: manifest.length,
      manifest,
      unreferenced: plan.unreferenced,
      assetDirectory: assets === "side-loaded" ? packaged?.assetDirectory ?? sideLoaded : null,
      bytes: (await stat(executable)).size,
      packaging: packaged,
      signed,
    };
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

import { access, copyFile, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { basename, dirname, extname, join, posix, relative, resolve, sep } from "node:path";
import { tmpdir } from "node:os";

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

function relativeAssetUrl(url, sourceFile) {
  const split = url.search(/[?#]/);
  const pathname = (split < 0 ? url : url.slice(0, split)).slice(1);
  const suffix = split < 0 ? "" : url.slice(split);
  let rewritten = posix.relative(posix.dirname(sourceFile), pathname);
  if (!rewritten) rewritten = ".";
  if (!rewritten.startsWith(".")) rewritten = `./${rewritten}`;
  return rewritten + suffix;
}

export function rewriteRootRelativeReferences(source, sourceFile) {
  const rewrite = (_match, prefix, url, suffix = "") =>
    `${prefix}${relativeAssetUrl(url, sourceFile)}${suffix}`;
  if ([".html", ".htm"].includes(extname(sourceFile).toLowerCase())) {
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

function launcherSource(files, options) {
  const imports = files.map((file, index) =>
    `import asset${index} from ${JSON.stringify(`./app/${file.relative}`)} with { type: "file" };`
  ).join("\n");
  const manifest = files.map((file, index) =>
    `{ path: ${JSON.stringify(file.relative)}, source: asset${index} }`
  ).join(",\n  ");
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
const root = await mkdtemp(join(tmpdir(), "blitsen-app-"));
const cleanup = () => rm(root, { recursive: true, force: true });
try {
  for (const asset of assets) {
    const destination = join(root, ...asset.path.split("/"));
    await mkdir(dirname(destination), { recursive: true });
    await writeFile(destination, await Bun.file(asset.source).arrayBuffer());
  }
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
    console.log("Blitsen standalone check passed (${files.length} embedded assets)");
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

export async function buildStandalone({ root, width, height, title, outfile, force = false }, nativePath) {
  if (!nativePath) throw new Error("native addon is unavailable; reinstall blitsen for this platform");
  await access(nativePath).catch(() => {
    throw new Error(`native addon is unavailable: ${nativePath}`);
  });
  const destination = resolve(outfile ?? defaultOutfile(root));
  if (!force) {
    const existing = await stat(destination).catch(() => null);
    if (existing) throw new Error(`output already exists: ${destination} (pass --force to replace it)`);
  }

  const files = await collectFiles(root);
  if (!files.some(file => file.relative === "index.html")) {
    throw new Error(`missing application entrypoint: ${join(root, "index.html")}`);
  }
  const staging = await mkdtemp(join(tmpdir(), "blitsen-build-"));
  try {
    await mkdir(join(staging, "app"), { recursive: true });
    for (const file of files) {
      const staged = join(staging, "app", ...file.relative.split("/"));
      await mkdir(dirname(staged), { recursive: true });
      if ([".html", ".htm", ".css"].includes(extname(file.relative).toLowerCase())) {
        const source = await readFile(file.absolute, "utf8");
        await writeFile(staged, rewriteRootRelativeReferences(source, file.relative));
      } else {
        await copyFile(file.absolute, staged);
      }
    }
    await copyFile(nativePath, join(staging, "blitsen.node"));
    const launcher = join(staging, "launcher.mjs");
    await writeFile(launcher, launcherSource(files, { width, height, title }));
    await mkdir(dirname(destination), { recursive: true });
    const result = await Bun.build({
      entrypoints: [launcher],
      compile: { outfile: destination },
    });
    if (!result.success) {
      const detail = result.logs.map(log => String(log)).join("\n");
      throw new Error(`standalone compilation failed${detail ? `:\n${detail}` : ""}`);
    }
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
  return { outfile: destination, assets: files.length, bytes: (await stat(destination)).size };
}

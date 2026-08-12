import { describe, expect, test } from "bun:test";
import { copyFile, cp, mkdir, mkdtemp, readFile, readdir, realpath, rm, stat, writeFile }
  from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import { buildManifest, generateApiManifest, loadApiManifest, renderCompatibilityDoc }
  from "../src/api-manifest.mjs";
import { createReloadCoordinator, main, packageVersion, parseArgs, resolveApplication } from "../src/cli.mjs";
import { CONFIG_SCHEMA, defineConfig, loadConfig, runBuildCommand, validateConfig } from "../src/config.mjs";
import { doctorApplication } from "../src/doctor.mjs";
import { buildStandalone, describeNativeBinary, planIngest, rewriteRootRelativeReferences,
  runtimeRecord } from "../src/export.mjs";
import { packageBuild, signArgv, signArtifact } from "../src/packaging.mjs";
import { describeRuntime, hostTarget, resolveRuntime, TARGETS } from "../src/runtime.mjs";

const runtimeSource = join(import.meta.dir, "../../../crates/blitsen-node/src/dom_bridge.rs");
const viteBase = join(import.meta.dir, "fixtures/vite-base");
const configFixtures = join(import.meta.dir, "fixtures/config");
const addonFixtures = join(import.meta.dir, "fixtures/addons");
const icon = join(import.meta.dir, "fixtures/icons/app-256.png");
const signHook = `sh ${join(import.meta.dir, "fixtures/sign/record-artifact.sh")}`;

// Nothing about the addon path can be proven with a stand-in file: dlopen is what
// decides. The fixtures are C, so a host compiler is what gates these tests, and
// the Windows toolchain needs an import library the fixture deliberately lacks.
const compiler = process.platform === "win32" ? null : (Bun.which("cc") ?? Bun.which("gcc"));
const engineAddon = join(import.meta.dir, "../../../target/release", {
  linux: "libblitsen_node.so", darwin: "libblitsen_node.dylib", win32: "blitsen_node.dll",
}[process.platform] ?? "missing");
// Proving a real load needs a real engine to launch. `bun run test:standalone`
// builds it; without it the exported-executable test declares itself skipped
// rather than pretending the export path was exercised.
const engineBuilt = await Bun.file(engineAddon).exists();
const platformPackages = join(import.meta.dir, "../../platforms");
const cliVersion = await packageVersion();

// A real node_modules tree, so resolution is proven through the resolver every package
// manager writes for, against the committed manifests rather than a copy of them.
async function withPlatformPackages(installed, run) {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-runtime-"));
  try {
    for (const [target, { version, binary = true }] of Object.entries(installed)) {
      const packaged = join(directory, "node_modules/@blitsen", target);
      await mkdir(packaged, { recursive: true });
      const manifest = JSON.parse(
        await readFile(join(platformPackages, target, "package.json"), "utf8"));
      await writeFile(join(packaged, "package.json"), JSON.stringify({ ...manifest, version }));
      if (binary) await writeFile(join(packaged, "blitsen.node"), "// placeholder addon\n");
    }
    return await run({ directory, require: createRequire(join(directory, "resolve.mjs")) });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

function compileAddon(directory, source = "greet.c", name = "greet.node") {
  const output = join(directory, name);
  const result = Bun.spawnSync({
    cmd: [compiler, "-shared", "-fPIC", "-o", output, join(addonFixtures, source),
      ...process.platform === "darwin" ? ["-Wl,-undefined,dynamic_lookup"] : []],
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) throw new Error(`compiling ${source} failed: ${result.stderr}`);
  return output;
}

// A 64-byte ELF64 header is all describeNativeBinary reads, so a host-independent
// one stands in for an addon built somewhere else.
function elfHeader({ machine = 0xb7, type = 3 } = {}) {
  const header = Buffer.alloc(64);
  header.write("\x7fELF", 0, "binary");
  header[4] = 2;
  header[5] = 1;
  header[6] = 1;
  header.writeUInt16LE(type, 16);
  header.writeUInt16LE(machine, 18);
  return header;
}

// Bun.build --compile refuses to start without the addon file, but never loads
// it, so a placeholder is enough to exercise the whole export pipeline.
async function withStubbedExport(run) {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-export-test-"));
  const nativePath = join(directory, "blitsen.node");
  await writeFile(nativePath, "// placeholder addon\n");
  try {
    return await run({ directory, nativePath, outfile: join(directory, "App") });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

// Step ⑤ is file generation over an already-linked artifact, so the macOS and
// Windows layouts are exercised on any host by handing it a stand-in executable.
async function withArtifact(run, name = "Pong") {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-package-test-"));
  const executable = join(directory, name);
  await writeFile(executable, "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  try {
    return await run({ directory, executable });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

function capture() {
  const lines = [];
  return {
    lines,
    output: {
      log: (line) => lines.push(["out", line]),
      error: (line) => lines.push(["err", line]),
    },
  };
}

describe("directory CLI", () => {
  test("prints help", async () => {
    const { lines, output } = capture();
    expect(await main(["--help"], output)).toBe(0);
    expect(lines[0][1]).toContain("Usage: blitsen <directory>");
  });

  test("parses native window flags", () => {
    expect(parseArgs(["app", "--width", "1024", "--height", "720", "--title", "Demo"]))
      .toEqual({ command: "run", directory: "app", width: 1024, height: 720, title: "Demo" });
    expect(parseArgs(["build", "dist", "--outfile", "Demo", "--force"]))
      .toEqual({ command: "build", directory: "dist", width: 800, height: 600,
        title: "Blitsen", outfile: "Demo", force: true });
    expect(parseArgs(["doctor", "dist", "--json"]))
      .toEqual({ command: "doctor", directory: "dist", width: 800, height: 600,
        title: "Blitsen", json: true });
    expect(parseArgs(["build", "dist", "--include", "*.txt", "--include", "meta/**",
      "--assets", "side-loaded"]))
      .toEqual({ command: "build", directory: "dist", width: 800, height: 600,
        title: "Blitsen", include: ["*.txt", "meta/**"], assets: "side-loaded" });
    expect(() => parseArgs(["app", "--width", "nope"])).toThrow("positive integer");
    expect(() => parseArgs(["app", "--force"])).toThrow("only valid with build");
    expect(() => parseArgs(["doctor", "dist", "--outfile", "x"])).toThrow("not valid with doctor");
    expect(() => parseArgs(["build", "dist", "--assets", "inline"])).toThrow("embedded or side-loaded");
    expect(() => parseArgs(["app", "--include", "*.txt"])).toThrow("only valid with build");
    expect(parseArgs(["build", "dist", "--addon", "native/physics.node"]).addons)
      .toEqual([join(process.cwd(), "native/physics.node")]);
    expect(() => parseArgs(["app", "--addon", "physics.node"])).toThrow("only valid with build");
    expect(parseArgs(["build", "dist", "--icon", "app.png", "--bundle-id", "com.example.pong",
      "--app-version", "1.2.3", "--sign", "codesign -s ID"]))
      .toEqual({ command: "build", directory: "dist", width: 800, height: 600, title: "Blitsen",
        icon: "app.png", bundleId: "com.example.pong", appVersion: "1.2.3", sign: "codesign -s ID" });
    expect(() => parseArgs(["app", "--icon", "app.png"])).toThrow("only valid with build");
  });

  test("names the application once for the title, the output file and the metadata", () => {
    expect(parseArgs(["build", "dist", "--out", "Demo", "--name", "My App"]))
      .toEqual({ command: "build", directory: "dist", width: 800, height: 600,
        title: "My App", name: "My App", outfile: "Demo" });
    // An explicit --title wins over the name it would otherwise default to.
    expect(parseArgs(["build", "dist", "--name", "My App", "--title", "Window"]).title).toBe("Window");
    expect(parseArgs(["build", "dist", "--title", "Window", "--name", "My App"]).title).toBe("Window");
    expect(() => parseArgs(["app", "--name", "My App"])).toThrow("only valid with build");
  });

  test("refuses cross-target export instead of quietly building for the host", () => {
    const host = `${process.platform}-${process.arch}`;
    const other = host === "linux-x64" ? "darwin-arm64" : "linux-x64";
    expect(parseArgs(["build", "dist", "--target", host]).target).toBe(host);
    expect(() => parseArgs(["build", "dist", "--target", other]))
      .toThrow("is not supported yet");
    expect(() => parseArgs(["build", "dist", "--target", other])).toThrow("see issue #72");
    expect(() => parseArgs(["build", "dist", "--target", "sunos-x64"]))
      .toThrow("unknown --target sunos-x64");
  });

  test("requires a directory for every command except a configured build", () => {
    expect(parseArgs(["build"]))
      .toEqual({ command: "build", directory: null, width: 800, height: 600, title: "Blitsen" });
    expect(() => parseArgs(["doctor"])).toThrow("missing application directory");
    expect(() => parseArgs(["--width", "800"])).toThrow("missing application directory");
  });

  test("resolves an index", async () => {
    const fixture = join(import.meta.dir, "../../../spikes/s7/fixture");
    const app = await resolveApplication(fixture);
    expect(app.entrypoint.endsWith("fixture/index.html")).toBeTrue();
  });

  test("reports the manifest version rather than a literal", async () => {
    const { lines, output } = capture();
    expect(await main(["--version"], output)).toBe(0);
    expect(lines[0][1]).toBe(await packageVersion());
  });

  test("diagnoses output outside the strict compatibility profile", async () => {
    const fixtures = join(import.meta.dir, "fixtures/doctor");
    const compatible = await doctorApplication(join(fixtures, "compatible"));
    expect(compatible).toMatchObject({ profile: "v0-strict", errors: 0, warnings: 0, files: 3 });

    const { lines, output } = capture();
    expect(await main(["doctor", join(fixtures, "unsupported")], output)).toBe(1);
    expect(lines.some(([, line]) => line.includes("HTML_CANVAS") && line.includes("native viewport")))
      .toBeTrue();
    expect(lines.some(([, line]) =>
      line.includes("WEB_STORAGE_MEMORY") && line.includes("gone when the application exits")))
      .toBeTrue();
    expect(lines.at(-1)[1]).toContain("1 errors, 2 warnings");
  });

  // The severities the third-party evidence settled. A subresource the export
  // will not serve is answered with empty bytes, so the page renders without it;
  // a remote <script src> is refused by the loader and then nothing runs at all.
  test("grades a remote subresource a warning and a remote script an error", async () => {
    const fixtures = join(import.meta.dir, "fixtures/doctor");
    const subresource = await doctorApplication(join(fixtures, "remote-subresource"));
    expect(subresource).toMatchObject({ errors: 0, warnings: 5 });
    // A preconnect, a Google Fonts stylesheet, a remote <img>, a CSS url().
    expect(subresource.diagnostics.map(diagnostic => `${diagnostic.severity}:${diagnostic.code}`))
      .toEqual(["warning:WEB_XHR", "warning:ASSET_REMOTE", "warning:ASSET_REMOTE",
        "warning:ASSET_REMOTE", "warning:ASSET_REMOTE"]);

    const script = await doctorApplication(join(fixtures, "remote"));
    expect(script.diagnostics.map(diagnostic => `${diagnostic.severity}:${diagnostic.code}`))
      .toEqual(["error:ASSET_REMOTE_SCRIPT"]);
  });

  // Every one of these shapes is verbatim from an unmodified third-party build
  // that renders correctly. A reference is not a call, and the scan cannot see
  // the guard around it, so none of them may block a build.
  test("does not block a build on absent APIs a bundle feature-detects", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-guarded-test-"));
    try {
      await writeFile(join(directory, "app.js"), [
        `typeof XMLHttpRequest<"u"&&new XMLHttpRequest;`,
        `typeof ShadowRoot<"u"&&e instanceof ShadowRoot;`,
        `typeof customElements<"u"&&customElements.get(n);`,
        `try{document.cookie="theme=dark"}catch{}`,
        `typeof Worker<"u"&&new Worker(u);`,
        `if(t.getContext)t.getContext("2d");`,
        `e?window.open(u,"_blank"):0;`,
        `typeof indexedDB<"u"&&indexedDB.open("x");`,
      ].join("\n"));
      const report = await doctorApplication(directory);
      expect(report.errors).toBe(0);
      expect([...new Set(report.diagnostics.map(diagnostic => diagnostic.code))].sort())
        .toEqual(["WEB_CANVAS", "WEB_COMPONENTS", "WEB_COOKIE", "WEB_NAVIGATION", "WEB_STORAGE",
          "WEB_WORKER", "WEB_XHR"]);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("accepts the routing and fetch surface the runtime actually implements", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-doctor-test-"));
    try {
      await writeFile(join(directory, "app.js"), [
        `history.pushState({ page: 1 }, "", "/reports");`,
        `history.replaceState(null, "", location.pathname);`,
        `addEventListener("popstate", () => fetch("https://api.example.com/reports"));`,
        `fetch(endpoint).then(response => response.json());`,
      ].join("\n"));
      const report = await doctorApplication(directory);
      expect(report).toMatchObject({ errors: 0, warnings: 0 });

      await writeFile(join(directory, "app.js"), [
        `fetch("/api/reports");`,
        `location.reload();`,
        `new ReadableStream();`,
      ].join("\n"));
      const codes = (await doctorApplication(directory)).diagnostics
        .map(diagnostic => `${diagnostic.severity}:${diagnostic.code}`);
      expect(codes).toEqual(["error:WEB_FETCH", "warning:WEB_NAVIGATION", "warning:WEB_STREAM"]);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("keeps the API manifest and the documented tiers generated from the runtime source", async () => {
    const generated = await generateApiManifest();
    expect(generated).toEqual(await loadApiManifest());
    expect(await renderCompatibilityDoc(generated))
      .toBe(await readFile(join(import.meta.dir, "../../../docs/COMPATIBILITY.md"), "utf8"));
  });

  // The manifest is worth nothing unless it fails when it stops matching the
  // bootstrap, so each way the two can diverge is made to happen here.
  test("refuses a manifest the runtime source disagrees with", async () => {
    const source = await readFile(runtimeSource, "utf8");
    expect(buildManifest(source).apis.find(entry => entry.api === "Worker").status).toBe("absent");
    const implemented = source
      .replace('"Worker", "SharedWorker"', '"SharedWorker"')
      .replace("const globals = {", "const globals = {\n    Worker,");
    expect(buildManifest(implemented).apis.find(entry => entry.api === "Worker").status)
      .toBe("implemented");

    expect(() => buildManifest(source.replace("const globals = {", "const globals = {\n    speechSynthesis,")))
      .toThrow("installs speechSynthesis");
    expect(() => buildManifest(source.replace('"requestIdleCallback",', '"speechSynthesis", "requestIdleCallback",')))
      .toThrow("deletes speechSynthesis");
    expect(() => buildManifest(source.replace(", AbortSignal, fetch,", ", AbortSignal,")))
      .toThrow("fetch are absent from the runtime but not deleted");
    expect(() => buildManifest(source.replace("const globals = {", "const stubbed = {")))
      .toThrow("no longer declares const globals = {");
  });

  test("diagnoses what the manifest calls absent, and nothing it calls implemented", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-manifest-test-"));
    try {
      await writeFile(join(directory, "app.js"),
        "new Worker(url); customElements.define(); indexedDB.open('x'); window.open('/x');\n"
        + "localStorage.setItem('theme', 'dark');");
      const codes = (await doctorApplication(directory)).diagnostics.map(entry => entry.code);
      expect(codes.sort()).toEqual(["WEB_COMPONENTS", "WEB_NAVIGATION", "WEB_STORAGE",
        "WEB_STORAGE_MEMORY", "WEB_WORKER"]);

      const manifest = await loadApiManifest();
      await writeFile(join(directory, "app.js"), manifest.apis
        .filter(entry => entry.status === "implemented" && entry.kind === "global")
        .map(entry => `void ${entry.api};`).join("\n"));
      expect(await doctorApplication(directory)).toMatchObject({ errors: 0, warnings: 0 });
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("normalizes Vite root-relative HTML and CSS references during ingest", () => {
    expect(rewriteRootRelativeReferences(
      '<script src="/assets/app.js?v=1"></script><a href="/settings">x</a>',
      "index.html",
    )).toBe('<script src="./assets/app.js?v=1"></script><a href="/settings">x</a>');
    expect(rewriteRootRelativeReferences(
      '.hero{background:url("/assets/hero.png#main")}',
      "assets/app.css",
    )).toBe('.hero{background:url("./hero.png#main")}');
  });

  // The fixture is shaped like minified bundler output with base "/app/": an
  // unspaced side-effect import, a base-prefixed import(), a transitive
  // @import, and new URL(…, import.meta.url).
  test("walks the module and stylesheet graph from the HTML entrypoint", async () => {
    const plan = await planIngest(viteBase);
    expect(plan.files.map(file => file.relative)).toEqual([
      "assets/chunk-BASE.js",
      "assets/hero-BASE.png",
      "assets/index-BASE.css",
      "assets/index-BASE.js",
      "assets/lazy-BASE.js",
      "assets/panel.svg",
      "assets/route-BASE.js",
      "assets/theme.css",
      "index.html",
    ]);
    expect(plan.unreferenced).toEqual(["assets/index-BASE.js.map", "assets/orphan.txt"]);
  });

  // A bundler resolves `import hero from "./hero.png"` into a bare literal, and
  // builds code-split chunk paths out of an array, so neither leaves an import
  // edge. Both were silently dropped from the export before.
  test("follows asset literals a bundler resolved, and only those that exist", async () => {
    const plan = await planIngest(viteBase);
    const kept = plan.files.map(file => file.relative);
    expect(kept).toContain("assets/hero-BASE.png");
    expect(kept).toContain("assets/route-BASE.js");
    // The same file carries strings that look like paths and are not files.
    // Being bounded by the emitted output is what makes the guess safe.
    expect(kept).not.toContain("assets/index-BASE.js.map");
    expect(plan.unreferenced).toContain("assets/orphan.txt");
  });

  // An error names something the export will not carry. Refusing by default is
  // right; refusing with no way through makes a remote analytics tag fatal to an
  // application that otherwise runs perfectly.
  test("a compatibility error names the way through", () => {
    expect(parseArgs(["build", "dist", "--accept-errors"]).acceptErrors).toBeTrue();
    expect(() => parseArgs(["doctor", "dist", "--accept-errors"]))
      .toThrow("--accept-errors is only valid with build");
  });

  test("keeps unreferenced output that an --include glob asks for", async () => {
    const plan = await planIngest(viteBase, { include: ["assets/*.txt"] });
    expect(plan.files.some(file => file.relative === "assets/orphan.txt")).toBeTrue();
    expect(plan.files.some(file => file.relative === "assets/index-BASE.js.map")).toBeFalse();
    expect(plan.unreferenced).toEqual(["assets/index-BASE.js.map"]);
    const everything = await planIngest(viteBase, { include: ["**"] });
    expect(everything.files).toHaveLength(11);
    expect(everything.unreferenced).toEqual([]);
  });

  test("resolves a custom bundler base against the real output layout", async () => {
    const plan = await planIngest(viteBase);
    const resolutions = plan.resolutions.get("index.html");
    expect(resolutions.get("/app/assets/index-BASE.js")).toBe("assets/index-BASE.js");
    const source = await readFile(join(viteBase, "index.html"), "utf8");
    const rewritten = rewriteRootRelativeReferences(source, "index.html",
      path => resolutions.get(path) ?? null);
    expect(rewritten).toContain('src="./assets/index-BASE.js"');
    expect(rewritten).toContain('href="./assets/index-BASE.css"');
    // Navigation targets are not subresources and stay exactly as authored.
    expect(rewritten).toContain('<a href="/app/docs">');
  });

  test("fails the build on references it cannot resolve inside the output", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-missing-"));
    try {
      await writeFile(join(directory, "index.html"), '<link rel="stylesheet" href="/assets/gone.css">');
      await expect(planIngest(directory)).rejects.toThrow("index.html references /assets/gone.css");
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("refuses to build output with compatibility errors", async () => {
    const fixture = join(import.meta.dir, "fixtures/doctor/remote");
    let built = false;
    const { lines, output } = capture();
    const runtime = { build: async () => { built = true; return {}; } };
    expect(await main(["build", fixture, "--outfile", "/tmp/blitsen-never"], output, runtime)).toBe(1);
    expect(built).toBeFalse();
    // The blocking diagnostic names its file, on stderr, under the step that found it.
    expect(lines.some(([stream, line]) => stream === "err"
      && line.trimStart().startsWith("index.html:") && line.includes("ASSET_REMOTE_SCRIPT")))
      .toBeTrue();
    expect(lines.at(-1)[1]).toContain("1 compatibility error blocks this build");
  });

  test("builds output whose only diagnostics are warnings, and reports them", async () => {
    const fixture = join(import.meta.dir, "fixtures/doctor/remote-subresource");
    const { lines, output } = capture();
    const runtime = { build: async ({ outfile }) => ({ outfile, assets: 3, bytes: 1024 }) };
    expect(await main(["build", fixture, "--outfile", "/tmp/blitsen-never"], output, runtime))
      .toBe(0);
    expect(lines.some(([stream, line]) => stream === "out" && line.includes("ASSET_REMOTE")))
      .toBeTrue();
    expect(lines.every(([stream]) => stream === "out")).toBeTrue();
  });

  test("hashes collected assets and compiles the same input to identical bytes", async () => {
    await withStubbedExport(async ({ nativePath, outfile }) => {
      const options = { root: viteBase, width: 800, height: 600, title: "Base", outfile, force: true };
      const first = await buildStandalone(options, nativePath);
      const bytes = await readFile(outfile);
      const second = await buildStandalone(options, nativePath);

      expect(first.layout).toBe("embedded");
      expect(first.assets).toBe(9);
      expect(first.unreferenced).toEqual(["assets/index-BASE.js.map", "assets/orphan.txt"]);
      expect(first.manifest.every(asset => /^[0-9a-f]{64}$/.test(asset.hash))).toBeTrue();
      // The hash covers the staged copy, so it reflects the rewritten references.
      const staged = first.manifest.find(asset => asset.path === "assets/index-BASE.css");
      expect(staged.hash).toBe(new Bun.CryptoHasher("sha256")
        .update('#root { background: url("./panel.svg") }\n@import "./theme.css";\n')
        .digest("hex"));
      expect(second.manifest).toEqual(first.manifest);
      // Byte equality holds for one input directory, output path, working
      // directory and Bun version: Bun records the compiled entrypoint's path.
      expect(Buffer.compare(bytes, await readFile(outfile))).toBe(0);
    });
  });

  test("lays assets out beside the executable when asked", async () => {
    await withStubbedExport(async ({ nativePath, outfile }) => {
      const result = await buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Base", outfile, assets: "side-loaded",
      }, nativePath);
      expect(result.layout).toBe("side-loaded");
      expect(result.assetDirectory).toBe(`${outfile}.assets`);
      expect((await readdir(result.assetDirectory)).sort()).toEqual(["assets", "index.html"]);
      const side = await readFile(join(result.assetDirectory, "index.html"), "utf8");
      expect(side).toContain('src="./assets/index-BASE.js"');
      expect(new Bun.CryptoHasher("sha256").update(side).digest("hex"))
        .toBe(result.manifest.find(asset => asset.path === "index.html").hash);
    });
  });

  test("reads the container header a .node must have to load on this host", () => {
    expect(describeNativeBinary(elfHeader({ machine: 0x3e })))
      .toEqual({ format: "ELF", platform: "linux", architectures: ["x64"] });
    expect(describeNativeBinary(elfHeader({ machine: 0x1234 })).architectures).toEqual(["0x1234"]);
    // An executable, an archive or a text file renamed .node is not a library.
    expect(describeNativeBinary(elfHeader({ type: 2 }))).toBeNull();
    expect(describeNativeBinary(Buffer.alloc(64, 0x41))).toBeNull();
    expect(describeNativeBinary(Buffer.from("// placeholder addon\n"))).toBeNull();
  });

  test("refuses a .node it cannot load instead of exporting a launch crash", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      const build = addons => buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Base", outfile, force: true, addons,
      }, nativePath);
      const foreign = join(directory, "foreign.node");
      await writeFile(foreign, elfHeader({ machine: 0xb7 }));
      await expect(build([foreign])).rejects.toThrow("native addon foreign.node is built for "
        + `linux-arm64 (ELF), but this export runs on ${process.platform}-${process.arch}`);
      const text = join(directory, "notes.node");
      await writeFile(text, "not a library\n");
      await expect(build([text]))
        .rejects.toThrow("notes.node is not a native addon: a .node file must be an ELF");
      await expect(build([icon])).rejects.toThrow(`a native addon must be a .node file: ${icon}`);
      await expect(build([join(directory, "absent.node")]))
        .rejects.toThrow("native addon does not exist:");
      // Two addons cannot both claim one name in the application tree.
      const other = join(directory, "nested", "foreign.node");
      await mkdir(dirname(other), { recursive: true });
      await copyFile(foreign, other);
      await expect(build([foreign, other]))
        .rejects.toThrow("two native addons would both be exported as foreign.node");
    });
  });

  test.skipIf(!compiler)("refuses a host shared library that is not a Node-API addon", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      const plain = compileAddon(directory, "plain.c", "plain.node");
      await expect(buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Base", outfile, addons: [plain],
      }, nativePath)).rejects.toThrow("native addon plain.node does not export "
        + "napi_register_module_v1: Blitsen loads Node-API addons, not V8/NAN addons");
    });
  });

  test.skipIf(!compiler)("carries a declared addon into both asset layouts", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      // Declared from outside the ingested directory, which is where an addon
      // lives: node_modules/<package>/build/Release, target/release.
      const addon = compileAddon(directory);
      const events = [];
      const embedded = await buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Base", outfile,
        addons: [addon], progress: event => events.push(event),
      }, nativePath);
      expect(embedded.addons).toEqual(["greet.node"]);
      expect(embedded.assets).toBe(10);
      expect(embedded.manifest.find(asset => asset.path === "greet.node").native).toBeTrue();
      expect(events[0].notes[1]).toBe("carried 1 native addon: greet.node "
        + "(load one from a module script with createRequire(import.meta.url))");

      const side = await buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Base", assets: "side-loaded",
        outfile: join(directory, "Side"), addons: [addon],
      }, nativePath);
      // The staged bytes are the compiled library, unmodified: dlopen reads them.
      expect(Buffer.compare(await readFile(join(side.assetDirectory, "greet.node")),
        await readFile(addon))).toBe(0);
    });
  });

  test.skipIf(!compiler)("keeps a declared addon in its place inside the output", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      const root = join(directory, "dist");
      await cp(viteBase, root, { recursive: true });
      const addon = compileAddon(join(root, "assets"));
      const result = await buildStandalone({
        root, width: 800, height: 600, title: "Base", outfile, addons: [addon],
      }, nativePath);
      // The specifier the application was written against still resolves, and a
      // declared file is carried rather than reported as unreachable and dropped.
      expect(result.addons).toEqual(["assets/greet.node"]);
      expect(result.unreferenced).toEqual(["assets/index-BASE.js.map", "assets/orphan.txt"]);
    });
  });

  test.skipIf(!compiler || !engineBuilt)(
    "loads a carried addon from the exported executable", async () => {
      const workspace = await mkdtemp(join(tmpdir(), "blitsen-addon-export-"));
      try {
        const root = join(workspace, "app");
        await cp(join(addonFixtures, "app"), root, { recursive: true });
        const addon = compileAddon(workspace);
        const assertLoaded = `(() => {
          const text = document.getElementById("greeting").textContent;
          if (text !== "blitsen-addon-ok") throw new Error("addon did not load: " + text);
        })()`;
        for (const assets of ["embedded", "side-loaded"]) {
          const outfile = join(workspace, `AddonApp-${assets}`);
          const result = await buildStandalone({
            root, width: 400, height: 300, title: "Addon", outfile, assets, addons: [addon],
          }, engineAddon);
          expect(result.addons).toEqual(["greet.node"]);
          // Run from an unrelated directory: the addon is found through the export,
          // not through the working directory it happened to be built in.
          const run = Bun.spawnSync({
            cmd: [result.outfile],
            cwd: tmpdir(),
            env: {
              PATH: "",
              BLITSEN_STANDALONE_CHECK: "1",
              BLITSEN_STANDALONE_CHECK_DELAY: "250",
              BLITSEN_STANDALONE_CHECK_ASSERT: assertLoaded,
            },
            stdout: "pipe",
            stderr: "pipe",
          });
          expect(run.stderr.toString()).toBe("");
          expect(run.exitCode).toBe(0);
        }
      } finally {
        await rm(workspace, { recursive: true, force: true });
      }
    }, 120_000);

  test("packages a Linux desktop entry, icon and signature into the build", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      const events = [];
      const result = await buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Pong Deluxe", outfile,
        icon, sign: signHook, platform: "linux", progress: event => events.push(event),
      }, nativePath);
      expect(result.outfile).toBe(outfile);
      // Steps ③–⑤ announce themselves as they finish, with what they produced.
      expect(events.map(event => event.step)).toEqual(["collect", "link", "package"]);
      expect(events[0].detail).toBe("9 embedded assets");
      expect(events[0].notes[0]).toBe("dropped 2 files unreachable from index.html "
        + "(--include <glob> keeps them): assets/index-BASE.js.map, assets/orphan.txt");
      expect(events[1].detail).toBe(outfile);
      expect(events[2].detail).toBe(`linux: ${result.packaging.artifacts.join(", ")}`);
      expect(events[2].notes).toEqual([`signed ${outfile} with: ${signHook}`]);
      expect(result.packaging.artifacts)
        .toEqual([join(directory, "App.desktop"), join(directory, "App.png")]);
      const entry = await readFile(join(directory, "App.desktop"), "utf8");
      expect(entry).toContain("[Desktop Entry]\nType=Application\n");
      expect(entry).toContain("Name=Pong Deluxe\n");
      expect(entry).toContain(`Exec=${outfile}\n`);
      expect(entry).toContain(`Icon=${join(directory, "App.png")}\n`);
      // Linux takes the PNG as it is; only Windows and macOS need a container.
      expect(Buffer.compare(await readFile(join(directory, "App.png")), await readFile(icon))).toBe(0);
      expect(result.signed).toEqual({ command: signHook, artifact: outfile });
      expect(await readFile(`${outfile}.signed`, "utf8")).toBe(`${outfile}\n`);
    });
  });

  test("quotes a desktop Exec holding reserved characters and omits an absent icon", async () => {
    await withArtifact(async ({ directory, executable }) => {
      await packageBuild({ platform: "linux", executable, title: "Pong & Co" });
      const entry = await readFile(join(directory, "Pong Deluxe.desktop"), "utf8");
      expect(entry).toContain(`Exec="${executable}"\n`);
      expect(entry).toContain("Name=Pong & Co\n");
      expect(entry).not.toContain("Icon=");
    }, "Pong Deluxe");
  });

  test("produces a macOS .app bundle with an Info.plist, PkgInfo and .icns", async () => {
    await withArtifact(async ({ directory, executable }) => {
      const sideLoaded = join(directory, "Pong.assets");
      await mkdir(sideLoaded);
      await writeFile(join(sideLoaded, "index.html"), "<!doctype html>");
      const result = await packageBuild({
        platform: "darwin", executable, title: "Pong Deluxe", icon,
        version: "1.2.3", assetDirectory: sideLoaded,
      });
      const bundle = join(directory, "Pong.app");
      expect(result).toMatchObject({
        bundle,
        executable: join(bundle, "Contents/MacOS/Pong"),
        assetDirectory: join(bundle, "Contents/MacOS/Pong.assets"),
        artifacts: [bundle],
        notes: [],
      });
      expect((await readdir(join(bundle, "Contents"))).sort())
        .toEqual(["Info.plist", "MacOS", "PkgInfo", "Resources"]);
      // Side-loaded assets resolve from the executable's directory, so they move
      // into the bundle with it.
      expect(await readdir(result.assetDirectory)).toEqual(["index.html"]);
      expect((await stat(result.executable)).mode & 0o111).toBeGreaterThan(0);
      expect(await readFile(join(bundle, "Contents/PkgInfo"), "utf8")).toBe("APPL????");

      const plist = await readFile(join(bundle, "Contents/Info.plist"), "utf8");
      expect(plist.startsWith('<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC')).toBeTrue();
      expect(plist).toContain("<key>CFBundleExecutable</key>\n  <string>Pong</string>");
      expect(plist).toContain("<key>CFBundleIdentifier</key>\n  <string>com.blitsen.pong-deluxe</string>");
      expect(plist).toContain("<key>CFBundleIconFile</key>\n  <string>Pong.icns</string>");
      expect(plist).toContain("<key>CFBundlePackageType</key>\n  <string>APPL</string>");
      expect(plist).toContain("<key>CFBundleShortVersionString</key>\n  <string>1.2.3</string>");
      expect(plist).toContain("<key>NSHighResolutionCapable</key>\n  <true/>");
      expect(plist.trimEnd().endsWith("</plist>")).toBeTrue();

      const png = await readFile(icon);
      const icns = await readFile(join(bundle, "Contents/Resources/Pong.icns"));
      expect(icns.subarray(0, 4).toString("ascii")).toBe("icns");
      expect(icns.readUInt32BE(4)).toBe(icns.length);
      expect(icns.subarray(8, 12).toString("ascii")).toBe("ic08");
      expect(icns.readUInt32BE(12)).toBe(png.length + 8);
      expect(Buffer.compare(icns.subarray(16), png)).toBe(0);
    });
  });

  test("writes a Windows application manifest and .ico beside the executable", async () => {
    await withArtifact(async ({ directory, executable }) => {
      const result = await packageBuild({
        platform: "win32", executable, title: "Pong Deluxe", icon, version: "1.2.3",
      });
      expect(result.artifacts)
        .toEqual([`${executable}.manifest`, join(directory, "Pong.ico")]);
      expect(result.executable).toBe(executable);
      expect(result.notes[0]).toContain("not embedded in the executable");

      const manifest = await readFile(`${executable}.manifest`, "utf8");
      expect(manifest).toContain('<assemblyIdentity type="win32" name="pong-deluxe" version="1.2.3.0"/>');
      expect(manifest).toContain("<description>Pong Deluxe</description>");
      expect(manifest).toContain('<requestedExecutionLevel level="asInvoker" uiAccess="false"/>');
      expect(manifest).toContain(">permonitorv2,permonitor</dpiAwareness>");
      expect(manifest).toContain(">UTF-8</activeCodePage>");
      expect(manifest.trimEnd().endsWith("</assembly>")).toBeTrue();

      const png = await readFile(icon);
      const ico = await readFile(join(directory, "Pong.ico"));
      expect([ico.readUInt16LE(0), ico.readUInt16LE(2), ico.readUInt16LE(4)]).toEqual([0, 1, 1]);
      // 256 pixels is stored as 0 in the single-byte width and height fields.
      expect([ico[6], ico[7], ico.readUInt16LE(12)]).toEqual([0, 0, 32]);
      expect(ico.readUInt32LE(14)).toBe(png.length);
      expect(ico.readUInt32LE(18)).toBe(22);
      expect(Buffer.compare(ico.subarray(22), png)).toBe(0);
    });
  });

  test("refuses icons and hosts it cannot package, and existing artifacts", async () => {
    await withArtifact(async ({ directory, executable }) => {
      await expect(packageBuild({ platform: "sunos", executable, title: "Pong" }))
        .rejects.toThrow("packaging is not supported on sunos");
      await expect(packageBuild({ platform: "linux", executable, title: "Pong", icon: "app.icns" }))
        .rejects.toThrow("linux icons must be .png or .svg");
      await expect(packageBuild({
        platform: "darwin", executable, title: "Pong",
        icon: join(import.meta.dir, "fixtures/icons/app-16.png"),
      })).rejects.toThrow("macOS icons need a PNG of 128, 256, 512, 1024 pixels");
      await expect(packageBuild({ platform: "linux", executable, title: "Pong", icon: "missing.png" }))
        .rejects.toThrow("icon file does not exist: missing.png");
      // A refused icon is refused before anything is written.
      expect(await readdir(directory)).toEqual(["Pong"]);

      await packageBuild({ platform: "linux", executable, title: "Pong", icon });
      await expect(packageBuild({ platform: "linux", executable, title: "Pong", icon }))
        .rejects.toThrow(`output already exists: ${join(directory, "Pong.desktop")}`);
      await packageBuild({ platform: "linux", executable, title: "Pong", icon, force: true });
    });
  });

  test("hands the signing hook the artifact and fails the build when it rejects it", async () => {
    expect(signArgv("darwin", "codesign -s ID", "/out/My App.app"))
      .toEqual(["sh", "-c", 'codesign -s ID "$@"', "sh", "/out/My App.app"]);
    expect(signArgv("win32", "signtool sign", "C:\\out\\App.exe"))
      .toEqual(["cmd", "/c", 'signtool sign "C:\\out\\App.exe"']);
    await withArtifact(async ({ executable }) => {
      // The hook is a command, not a shell fragment we interpolate into: the
      // artifact arrives as one positional argument however it is spelled.
      const bundle = `${executable} Deluxe.app`;
      await writeFile(bundle, "");
      expect(await signArtifact({ platform: "linux", command: signHook, artifact: bundle }))
        .toEqual({ command: signHook, artifact: bundle });
      expect(await readFile(`${bundle}.signed`, "utf8")).toBe(`${bundle}\n`);
      await expect(signArtifact({ platform: "linux", command: "false", artifact: executable }))
        .rejects.toThrow("signing command failed with exit code 1: false");
    });
  });

  test("opens a resolved directory through the native runtime", async () => {
    const fixture = join(import.meta.dir, "../../../spikes/s7/fixture");
    let opened;
    let pumps = 0;
    const runtime = {
      openDirectory: async (options) => { opened = options; },
      pumpWindow: () => ++pumps < 3,
      waitForNextFrame: async () => {},
    };
    expect(await main([fixture, "--title", "Fixture"], console, runtime)).toBe(0);
    expect(opened.title).toBe("Fixture");
    expect(opened.entrypoint.endsWith("index.html")).toBeTrue();
    expect(pumps).toBe(3);
  });

  test("builds a resolved directory through the standalone exporter, naming each step", async () => {
    const fixture = join(import.meta.dir, "../../../examples/pong");
    let built;
    const runtime = {
      build: async options => {
        built = options;
        options.progress({ step: "collect", detail: "3 embedded assets", notes: ["dropped 1 file"] });
        options.progress({ step: "link", detail: "/tmp/pong" });
        options.progress({ step: "package", detail: "linux: /tmp/pong.desktop", notes: ["note"] });
        return { outfile: "/tmp/pong", assets: 3, bytes: 123 };
      },
    };
    const { lines, output } = capture();
    expect(await main(["build", fixture, "--outfile", "/tmp/pong", "--icon", "app.png",
      "--sign", "codesign"], output, runtime)).toBe(0);
    expect(built.command).toBe("build");
    expect(built.entrypoint.endsWith("examples/pong/index.html")).toBeTrue();
    expect(built.icon).toBe("app.png");
    expect(lines[0][1]).toBe(`① ingest  ${built.entrypoint}`);
    expect(lines[1][1]).toMatch(/^② scan {4}\d+ files, 0 errors, \d+ warnings$/);
    const steps = lines.map(([, line]) => line).filter(line => /^[⓪①②③④⑤]/.test(line));
    expect(steps.slice(2)).toEqual([
      "③ collect 3 embedded assets",
      "④ link    /tmp/pong",
      "⑤ package linux: /tmp/pong.desktop",
    ]);
    expect(lines.some(([, line]) => line === "          dropped 1 file")).toBeTrue();
    expect(lines.some(([, line]) => line === "          note")).toBeTrue();
    expect(lines.at(-2)[1]).toBe("Built /tmp/pong (3 assets, 123 bytes)");
    expect(lines.at(-1)[1]).toContain("not yet cleared for redistribution");
  });

  test("names the offending file and exits non-zero when ingest cannot resolve a reference", async () => {
    await withStubbedExport(async ({ directory, nativePath, outfile }) => {
      const root = join(directory, "app");
      await mkdir(root);
      await writeFile(join(root, "index.html"), '<link rel="stylesheet" href="/assets/gone.css">');
      const { lines, output } = capture();
      const runtime = { build: options => buildStandalone(options, nativePath) };
      expect(await main(["build", root, "--out", outfile], output, runtime)).toBe(1);
      expect(lines.at(-1)).toEqual(["err", "blitsen: unresolved local references in the "
        + "application output:\n  index.html references /assets/gone.css"]);
      expect(await stat(outfile).catch(() => null)).toBeNull();
    });
  });

  test("yields timer macrotasks and microtasks before the next native pump", async () => {
    const fixture = join(import.meta.dir, "../../../spikes/s7/fixture");
    const order = [];
    let pumps = 0;
    const runtime = {
      openDirectory: async () => {},
      pumpWindow: () => {
        order.push(`pump:${++pumps}`);
        return pumps < 2;
      },
      waitForNextFrame: () => new Promise(resolve => setTimeout(() => {
        order.push("timer");
        Promise.resolve().then(() => order.push("microtask"));
        resolve();
      }, 0)),
    };
    expect(await main([fixture], console, runtime)).toBe(0);
    expect(order).toEqual(["pump:1", "timer", "microtask", "pump:2"]);
  });

  test("subtracts render work from the 60 Hz wait budget", async () => {
    const fixture = join(import.meta.dir, "../../../spikes/s7/fixture");
    const waits = [];
    let pumps = 0;
    const originalNow = performance.now;
    const times = [0, 6, 20, 41];
    performance.now = () => times.shift() ?? 41;
    try {
      const runtime = {
        openDirectory: async () => {},
        pumpWindow: () => ++pumps < 3,
        waitForNextFrame: async delay => waits.push(delay),
      };
      expect(await main([fixture], console, runtime)).toBe(0);
    } finally {
      performance.now = originalNow;
    }
    expect(waits[0]).toBeCloseTo(1000 / 60 - 6, 5);
    expect(waits[1]).toBeCloseTo(1000 / 30 - 20, 5);
  });

  test("debounces CSS swaps and escalates mixed batches to one document reload", async () => {
    const calls = [];
    const coordinator = createReloadCoordinator({
      reloadCSS: async file => calls.push(["css", file]),
      reloadDirectory: async () => calls.push(["document"]),
    }, console, 5);
    coordinator.notify("styles/app.css");
    coordinator.notify("styles/app.css");
    coordinator.notify("styles/theme.CSS");
    await Bun.sleep(15);
    await coordinator.settled();
    expect(calls).toEqual([["css", "styles/app.css"], ["css", "styles/theme.CSS"]]);

    coordinator.notify("styles/app.css");
    coordinator.notify("src/app.js");
    coordinator.notify("index.html");
    await Bun.sleep(15);
    await coordinator.settled();
    expect(calls.at(-1)).toEqual(["document"]);
    expect(calls.filter(call => call[0] === "document")).toHaveLength(1);
    coordinator.close();
  });

  test("falls back to a document reload when no stylesheet link matched", async () => {
    const calls = [];
    const coordinator = createReloadCoordinator({
      reloadCSS: async file => { calls.push(["css", file]); return false; },
      reloadDirectory: async () => calls.push(["document"]),
    }, console, 5);
    coordinator.notify("styles/imported.css");
    await Bun.sleep(15);
    await coordinator.settled();
    expect(calls).toEqual([["css", "styles/imported.css"], ["document"]]);
    coordinator.close();
  });

  test("publishes the schema it validates against", async () => {
    const published = join(import.meta.dir, "../src/config.schema.json");
    expect(JSON.parse(await readFile(published, "utf8"))).toEqual(CONFIG_SCHEMA);
    expect(defineConfig({ build: "vite build", output: "dist", name: "My App" }))
      .toEqual({ build: "vite build", output: "dist", name: "My App" });
  });

  test("rejects a malformed config naming the key and the file it came from", async () => {
    expect(() => defineConfig({ output: 7 }))
      .toThrow('invalid blitsen config in defineConfig(): "output" must be a string, found a number');
    expect(() => validateConfig({ output: "dist", name: " " }, "/app/package.json"))
      .toThrow('invalid blitsen config in /app/package.json: "name" must not be empty');
    expect(() => validateConfig({}, "/app/package.json"))
      .toThrow('invalid blitsen config in /app/package.json: missing required key "output"');
    expect(() => validateConfig(["dist"], "/app/package.json"))
      .toThrow("invalid blitsen config in /app/package.json: expected an object, found an array");
    expect(() => defineConfig({ output: "dist", addons: "physics.node" }))
      .toThrow('invalid blitsen config in defineConfig(): "addons" must be an array, found a string');
    expect(() => defineConfig({ output: "dist", addons: ["a.node", 7] }))
      .toThrow('invalid blitsen config in defineConfig(): "addons[1]" must be a string, found a number');
    expect(defineConfig({ output: "dist", addons: ["native/physics.node"] }).addons)
      .toEqual(["native/physics.node"]);
    const misspelled = join(configFixtures, "misspelled");
    await expect(loadConfig(misspelled)).rejects.toThrow(
      `invalid blitsen config in ${join(misspelled, "package.json")}: `
      + 'unknown key "outputs" (known keys: build, output, name, addons)');
  });

  test("discovers the config in the nearest package.json declaring it", async () => {
    const found = await loadConfig(join(configFixtures, "wrapped"));
    expect(found.root).toBe(join(configFixtures, "wrapped"));
    expect(found.config).toEqual({ build: "node emit-dist.mjs", output: "dist",
      name: "Wrapped App", addons: ["native/greet.node"] });
    // A package.json without the key is not a config, and neither is no package.json.
    const bare = await mkdtemp(join(tmpdir(), "blitsen-config-"));
    try {
      expect(await loadConfig(bare)).toEqual({ path: null, root: null, config: null });
      await writeFile(join(bare, "package.json"), '{"name":"bare"}');
      expect(await loadConfig(bare))
        .toEqual({ path: join(bare, "package.json"), root: null, config: null });
      await writeFile(join(bare, "package.json"), "{ not json");
      await expect(loadConfig(bare)).rejects.toThrow("package.json is not valid JSON");
    } finally {
      await rm(bare, { recursive: true, force: true });
    }
  });

  test("fails the build when the configured command does", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-command-"));
    try {
      await expect(runBuildCommand("exit 3", directory))
        .rejects.toThrow("build command failed with exit code 3: exit 3");
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("runs the configured build and ingests the directory it wrote", async () => {
    const workspace = await mkdtemp(join(tmpdir(), "blitsen-wrapped-"));
    const project = join(workspace, "app");
    await cp(join(configFixtures, "wrapped"), project, { recursive: true });
    const cwd = process.cwd();
    let built;
    try {
      process.chdir(project);
      const here = process.cwd();
      const { lines, output } = capture();
      const runtime = {
        build: async options => {
          built = options;
          return { outfile: options.outfile, assets: 1, bytes: 1 };
        },
      };
      expect(await main(["build"], output, runtime)).toBe(0);
      expect(lines[0][1])
        .toBe(`⓪ build   node emit-dist.mjs (configured in ${join(project, "package.json")})`);
      // The command really ran: Blitsen only knows the directory it left behind.
      expect(await readFile(join(project, "dist/index.html"), "utf8")).toContain("wrapped");
      expect(built.root).toBe(await realpath(join(project, "dist")));
      expect(built.title).toBe("Wrapped App");
      expect(built.outfile).toBe(join(here, "Wrapped App"));
      // Configured addon paths are the user's, relative to their package.json.
      expect(built.addons).toEqual([join(project, "native/greet.node")]);
    } finally {
      process.chdir(cwd);
      await rm(workspace, { recursive: true, force: true });
    }
  });

  test("asks for a directory or a config when neither is there", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-unconfigured-"));
    const cwd = process.cwd();
    try {
      process.chdir(directory);
      const { lines, output } = capture();
      expect(await main(["build"], output, { build: async () => ({}) })).toBe(1);
      expect(lines[0][1]).toContain('pass one, or add a "blitsen" config to');
      expect(lines[0][1]).toContain("package.json");
    } finally {
      process.chdir(cwd);
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("reports missing entrypoints and unavailable native addons", async () => {
    const { lines, output } = capture();
    expect(await main([import.meta.dir], output, {})).toBe(1);
    expect(lines[0][1]).toContain("missing or unreadable entrypoint");
  });
});

describe("runtime resolution", () => {
  test("declares one platform package per target, pinned to this version exactly", async () => {
    const manifest = JSON.parse(await readFile(join(import.meta.dir, "../package.json"), "utf8"));
    expect(Object.keys(manifest.optionalDependencies))
      .toEqual(TARGETS.map(target => `@blitsen/${target}`));
    for (const target of TARGETS) {
      const [os, cpu] = target.split("-");
      const platform = JSON.parse(
        await readFile(join(platformPackages, target, "package.json"), "utf8"));
      expect(platform.name).toBe(`@blitsen/${target}`);
      // os/cpu are what make a package manager install only the host's binary.
      expect(platform.os).toEqual([os]);
      expect(platform.cpu).toEqual([cpu]);
      expect(platform.exports["./blitsen.node"]).toBe("./blitsen.node");
      // A range would allow a pair that was never built together; see TECH.md §11.
      expect(platform.version).toBe(cliVersion);
      expect(manifest.optionalDependencies[platform.name]).toBe(cliVersion);
    }
  });

  test("picks the package matching the target, ignoring the others installed", async () => {
    await withPlatformPackages(
      { "linux-x64": { version: cliVersion }, "darwin-arm64": { version: cliVersion } },
      async ({ directory, require }) => {
        for (const target of ["linux-x64", "darwin-arm64"]) {
          expect(await resolveRuntime({ target, version: cliVersion, env: {}, require })).toEqual({
            path: join(directory, "node_modules/@blitsen", target, "blitsen.node"),
            target,
            version: cliVersion,
            package: `@blitsen/${target}`,
            source: "package",
          });
        }
      });
  });

  test("names the platform, and says it is unpublished, when its package is absent", async () => {
    await withPlatformPackages({ "linux-x64": { version: cliVersion } }, async ({ require }) => {
      const missing = resolveRuntime({ target: "win32-arm64", version: cliVersion, env: {}, require });
      await expect(missing).rejects.toThrow(
        "no Blitsen runtime for win32-arm64: @blitsen/win32-arm64 is not installed");
      await expect(missing)
        .rejects.toThrow("no platform runtime package is published yet and only linux-x64 is built");
      // A host outside the six is a different failure: nothing to install at all.
      await expect(resolveRuntime({ target: "freebsd-x64", version: cliVersion, env: {}, require }))
        .rejects.toThrow("Blitsen has no runtime for freebsd-x64: supported targets are darwin-arm64");
    });
  });

  test("refuses a platform package whose version is not the CLI's", async () => {
    await withPlatformPackages({ "linux-x64": { version: "0.0.9" } }, async ({ require }) => {
      await expect(resolveRuntime({ target: "linux-x64", version: "1.2.3", env: {}, require }))
        .rejects.toThrow("runtime version mismatch: blitsen 1.2.3 requires @blitsen/linux-x64 "
          + "1.2.3, but 0.0.9 is installed");
    });
  });

  test("refuses a platform package that carries no addon", async () => {
    await withPlatformPackages({ "linux-x64": { version: cliVersion, binary: false } },
      async ({ require }) => {
        await expect(resolveRuntime({ target: "linux-x64", version: cliVersion, env: {}, require }))
          .rejects.toThrow(`@blitsen/linux-x64@${cliVersion} is installed but carries no blitsen.node`);
      });
  });

  test("takes BLITSEN_NATIVE_PATH ahead of an installed package, in either spelling", async () => {
    await withPlatformPackages({ "linux-x64": { version: cliVersion } }, async ({ require }) => {
      const addon = join(tmpdir(), "explicit.node");
      for (const configured of [addon, pathToFileURL(addon).href]) {
        expect(await resolveRuntime({
          target: "linux-x64", version: cliVersion, require,
          env: { BLITSEN_NATIVE_PATH: configured },
        })).toEqual({ path: addon, target: "linux-x64", version: null, package: null,
          source: "environment" });
      }
    });
  });

  // The repository path every test script uses: an addon this checkout built itself,
  // found without an install and deliberately unversioned.
  test.skipIf(!engineBuilt)("falls back to a checkout's own build", async () => {
    await withPlatformPackages({}, async ({ require }) => {
      const resolved = await resolveRuntime({ version: cliVersion, env: {}, require });
      expect(resolved.source).toBe("repository");
      expect(resolved.target).toBe(hostTarget());
      expect(resolved.version).toBeNull();
      expect(await Bun.file(resolved.path).exists()).toBeTrue();
    });
  });

  test("records the runtime an export linked against, in the artifact and the report", async () => {
    const runtime = { target: "linux-x64", version: "1.2.3", package: "@blitsen/linux-x64",
      source: "package" };
    await withStubbedExport(async ({ nativePath, outfile }) => {
      const result = await buildStandalone(
        { root: viteBase, width: 800, height: 600, title: "Base", outfile },
        { ...runtime, path: nativePath });
      expect(result.runtime).toEqual({ ...runtime, path: nativePath });
      // The stamp survives the link, so a shipped executable names its own runtime.
      expect((await readFile(outfile)).includes(JSON.stringify(runtime))).toBeTrue();
    });
    const { lines, output } = capture();
    expect(await main(["build", join(import.meta.dir, "../../../examples/pong"),
      "--outfile", "/tmp/blitsen-never"], output,
    { build: async () => ({ outfile: "/tmp/pong", assets: 3, bytes: 123, runtime }) })).toBe(0);
    expect(lines.map(([, line]) => line)).toContain("Runtime: @blitsen/linux-x64@1.2.3");
  });

  test("describes a runtime that came from a path as unversioned", () => {
    const record = runtimeRecord("/tmp/blitsen.node");
    expect(record).toEqual({ path: "/tmp/blitsen.node", target: hostTarget(), version: null,
      package: null, source: "path" });
    expect(describeRuntime(record)).toBe(`${hostTarget()} (unversioned, from path)`);
    expect(() => runtimeRecord(null))
      .toThrow("native addon is unavailable; reinstall blitsen for this platform");
  });
});

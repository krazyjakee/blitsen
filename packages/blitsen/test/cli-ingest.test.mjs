import { describe, expect, test } from "bun:test";
import { mkdtemp, readFile, readdir, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { main, parseArgs } from "../src/cli.mjs";
import { buildStandalone, planIngest, rewriteRootRelativeReferences } from "../src/export.mjs";
import { viteBase, exportedName, withStubbedExport, capture } from "./cli-support.mjs";

describe("directory CLI", () => {
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

  test.skipIf(process.platform === "win32")("rejects symbolic links in application output", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-symlink-"));
    try {
      await writeFile(join(directory, "index.html"), "<html></html>");
      await symlink(join(directory, "index.html"), join(directory, "linked.html"));
      await expect(planIngest(directory))
        .rejects.toThrow("application output contains a symbolic link: linked.html");
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("refuses to build output with compatibility errors", async () => {
    const fixture = join(import.meta.dir, "fixtures/doctor/unsupported");
    let built = false;
    const { lines, output } = capture();
    const runtime = { build: async () => { built = true; return {}; } };
    expect(await main(["build", fixture, "--outfile", "/tmp/blitsen-never"], output, runtime)).toBe(1);
    expect(built).toBeFalse();
    // The blocking diagnostic names its file, on stderr, under the step that found it.
    expect(lines.some(([stream, line]) => stream === "err"
      && line.trimStart().startsWith("index.html:") && line.includes("HTML_CANVAS")))
      .toBeTrue();
    expect(lines.at(-1)[1]).toContain("1 compatibility error blocks this build");
  });

  // wordle-plus is a real application that builds only because of this: its one
  // diagnostic is a Google Analytics tag, which the runtime skips rather than
  // fetches. Blocking the export over it bought no privacy and cost a flag.
  test("builds output whose only error was a remote script", async () => {
    const fixture = join(import.meta.dir, "fixtures/doctor/remote");
    let built = false;
    const { lines, output } = capture();
    const runtime = { build: async ({ outfile }) => { built = true; return { outfile, assets: 2, bytes: 512 }; } };
    expect(await main(["build", fixture, "--outfile", "/tmp/blitsen-never"], output, runtime))
      .toBe(0);
    expect(built).toBeTrue();
    expect(lines.some(([stream, line]) => stream === "out" && line.includes("ASSET_REMOTE_SCRIPT")))
      .toBeTrue();
    expect(lines.every(([stream]) => stream === "out")).toBeTrue();
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
      const bytes = await readFile(first.outfile);
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
      expect(Buffer.compare(bytes, await readFile(second.outfile))).toBe(0);
    });
  });

  test("lays assets out beside the executable when asked", async () => {
    await withStubbedExport(async ({ nativePath, outfile }) => {
      const result = await buildStandalone({
        root: viteBase, width: 800, height: 600, title: "Base", outfile, assets: "side-loaded",
      }, nativePath);
      expect(result.layout).toBe("side-loaded");
      expect(result.assetDirectory).toBe(`${exportedName(outfile)}.assets`);
      expect((await readdir(result.assetDirectory)).sort()).toEqual(["assets", "index.html"]);
      const side = await readFile(join(result.assetDirectory, "index.html"), "utf8");
      expect(side).toContain('src="./assets/index-BASE.js"');
      expect(new Bun.CryptoHasher("sha256").update(side).digest("hex"))
        .toBe(result.manifest.find(asset => asset.path === "index.html").hash);
    });
  });

});

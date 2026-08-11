// Proves the specifier decision against real bundlers on default configs (issue #68).
//
// Three claims, measured rather than asserted from intuition:
//   1. `blitsen/<module>` resolves and bundles everywhere, with no configuration.
//   2. bare `native:<module>` does not survive a default config.
//   3. our optional plugin makes the bare form work, for users who prefer it.
//
// Claim 2 is the reason `blitsen/*` is recommended, and it is not uniform: esbuild,
// Vite, webpack and Bun fail outright, but Rollup only warns and externalizes. That
// silent pass is the worst outcome of the three — it produces a bundle whose import
// is unresolvable anywhere except inside Blitsen — so it is asserted explicitly
// rather than glossed as "bundlers reject it".
//
// Needs network on first run to install the bundlers into a temp directory.
import { strict as assert } from "node:assert";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repository = resolve(import.meta.dir, "../../..");
const packageRoot = join(repository, "packages/blitsen");
const workspace = await mkdtemp(join(tmpdir(), "blitsen-bundlers-"));
const out = entry => join(workspace, "out", entry);

const run = (cmd) =>
  Bun.spawnSync({ cmd, cwd: workspace, env: process.env, stdout: "pipe", stderr: "pipe" });
const output = result => `${result.stdout.toString()}${result.stderr.toString()}`;

// Each builds `src/<entry>.mjs`; `plugin` selects a config that loads our plugin.
const BUNDLERS = [
  {
    name: "esbuild",
    bare: "fails",
    build: (entry) =>
      ["npx", "esbuild", `src/${entry}.mjs`, "--bundle", "--format=esm", `--outfile=${out(entry)}-esbuild.mjs`],
  },
  {
    name: "rollup",
    bare: "warns and externalizes",
    build: (entry, plugin) => plugin
      ? ["npx", "rollup", "-c", "rollup.plugin.mjs"]
      : ["npx", "rollup", `src/${entry}.mjs`, "--format", "esm", "--file", `${out(entry)}-rollup.mjs`],
  },
  {
    name: "vite",
    bare: "fails",
    build: (entry, plugin) => ["npx", "vite", "build", "--logLevel", "error", "--ssr", `src/${entry}.mjs`,
      "--outDir", `${out(entry)}-vite`, ...(plugin ? ["-c", "vite.plugin.mjs"] : ["--configLoader", "native"])],
  },
  {
    name: "webpack",
    bare: "fails",
    build: (entry, plugin) => ["npx", "webpack", "--mode", "production", "--entry", `./src/${entry}.mjs`,
      "--output-path", `${out(entry)}-webpack`, "--stats", "errors-only",
      ...(plugin ? ["-c", "webpack.plugin.mjs"] : [])],
  },
  {
    name: "bun",
    bare: "fails",
    build: (entry) => [process.execPath, "build", `src/${entry}.mjs`, "--outfile", `${out(entry)}-bun.mjs`],
  },
];

try {
  await writeFile(join(workspace, "package.json"), JSON.stringify({
    name: "blitsen-bundler-matrix", private: true, type: "module",
  }));
  const install = run(["npm", "install", "--no-audit", "--no-fund", "--silent",
    "esbuild", "rollup", "webpack", "webpack-cli", "vite", `blitsen@file:${packageRoot}`]);
  if (install.exitCode !== 0) throw new Error(`bundler install failed:\n${output(install)}`);

  await mkdir(join(workspace, "src"), { recursive: true });
  await writeFile(join(workspace, "src/subpath.mjs"),
    'import dialog from "blitsen/dialog";\nexport default typeof dialog;\n');
  await writeFile(join(workspace, "src/bare.mjs"),
    'import dialog from "native:dialog";\nexport default typeof dialog;\n');
  await writeFile(join(workspace, "rollup.plugin.mjs"),
    'import { blitsenRollup } from "blitsen/bundler";\n'
    + 'export default { input: "src/bare.mjs", plugins: [blitsenRollup()],\n'
    + `  output: { file: ${JSON.stringify(`${out("bare")}-rollup-plugin.mjs`)}, format: "esm" } };\n`);
  await writeFile(join(workspace, "vite.plugin.mjs"),
    'import { blitsenVite } from "blitsen/bundler";\nexport default { plugins: [blitsenVite()] };\n');
  // ESM output is required: webpack's `module` external type needs it, and a Blitsen
  // application is ESM anyway. Without it webpack reports the target cannot use
  // dynamic import, which is a clearer failure than silently inlining the specifier.
  await writeFile(join(workspace, "webpack.plugin.mjs"),
    'import { blitsenWebpackExternals } from "blitsen/bundler";\n'
    + "export default { externals: [blitsenWebpackExternals()],\n"
    + '  experiments: { outputModule: true }, output: { module: true, chunkFormat: "module" } };\n');
  await writeFile(join(workspace, "esbuild.plugin.mjs"),
    'import { build } from "esbuild";\nimport { blitsenEsbuild } from "blitsen/bundler";\n'
    + `await build({ entryPoints: ["src/bare.mjs"], bundle: true, format: "esm",\n`
    + `  outfile: ${JSON.stringify(`${out("bare")}-esbuild-plugin.mjs`)}, plugins: [blitsenEsbuild()] });\n`);

  const summary = [];
  for (const bundler of BUNDLERS) {
    const resolved = run(bundler.build("subpath", false));
    assert.equal(resolved.exitCode, 0,
      `${bundler.name} could not resolve blitsen/dialog on a default config:\n${output(resolved)}`);

    const bare = run(bundler.build("bare", false));
    if (bundler.bare === "fails") {
      assert.notEqual(bare.exitCode, 0,
        `${bundler.name} was expected to reject a bare native: specifier by default`);
    } else {
      assert.equal(bare.exitCode, 0, `${bundler.name} behaviour changed: expected ${bundler.bare}`);
      assert.match(output(bare), /[Uu]nresolved/,
        "Rollup is expected to warn about the unresolved dependency, not accept it quietly");
    }
    summary.push(`${bundler.name}: subpath ok, bare ${bundler.bare}`);
  }

  // Claim 3 — the plugin makes the bare spelling work. Bun has no plugin here: its
  // bundler takes plugins only through the JS API, and Bun is the runtime rather
  // than a user's bundler, so the case does not arise.
  const withPlugin = [
    ["esbuild", ["npx", "node", "esbuild.plugin.mjs"]],
    ["rollup", BUNDLERS[1].build("bare", true)],
    ["vite", BUNDLERS[2].build("bare", true)],
    ["webpack", BUNDLERS[3].build("bare", true)],
  ];
  for (const [name, command] of withPlugin) {
    const result = run(command);
    assert.equal(result.exitCode, 0,
      `the ${name} plugin did not make a bare native: specifier build:\n${output(result)}`);
  }

  console.log(`Bundler matrix (default configs):\n  ${summary.join("\n  ")}`);
  console.log(`Plugin restores the bare form for: ${withPlugin.map(([name]) => name).join(", ")}.`);
} finally {
  await rm(workspace, { recursive: true, force: true });
}

// The M3b adoption gate on applications we did not write (issue #69).
//
// `test:m3b` builds `examples/vite-react`, an application authored in this
// repository whose markup carries the very hooks that gate queries. It proves the
// export pipeline, not adoption. This script is the adoption claim: clone real
// Vite applications at pinned revisions, build them with their own unmodified
// build command, and put that output through `doctor`, `build`, and the renderer.
//
// Deliberately opt-in and deliberately not in CI: it needs network, npm, pnpm via
// corepack, and several minutes of third-party builds. CI must not depend on
// cloning other people's repositories.
//
//   bun run --cwd packages/blitsen test:third-party [--only <name>] [--work <dir>]
//
// Zero source changes are made to any fixture. The only post-processing is on our
// own capture: `<script>` elements are stripped from the serialized post-JS DOM so
// the paint pass does not run the application a second time.
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, extname, join } from "node:path";
import { planIngest, rewriteRootRelativeReferences } from "../src/export.mjs";
import { buildAddon, repository } from "./build-addon.mjs";

const WIDTH = 1280;
const HEIGHT = 800;
// The exported launcher mounts by running the document's scripts and then letting
// the host event loop turn: React, Vue and Svelte all schedule their first render
// off a microtask or timer. Anything still empty after this settle never mounted.
const SETTLE_MS = 2000;
// Calibrated against examples/vite-react, which mounts 17 elements below <body>
// and paints 16 colours. A shell-only document has one element and one colour.
const MOUNTED_ELEMENTS = 10;
const PAINTED_COLOURS = 3;

const FIXTURES = [
  {
    name: "shadcn-admin",
    framework: "React 19",
    repository: "https://github.com/satnaing/shadcn-admin.git",
    revision: "70cfd3098f219f09a3c6941b2d1fabe4665dfa3d",
    licence: "MIT",
    install: ["corepack", "pnpm", "install", "--frozen-lockfile",
      "--config.dangerously-allow-all-builds=true"],
    build: ["corepack", "pnpm", "build"],
  },
  {
    name: "vue3-realworld",
    framework: "Vue 3",
    repository: "https://github.com/mutoe/vue3-realworld-example-app.git",
    revision: "a3b07312d4c416c3976a3012e64cf39053060708",
    licence: "MIT",
    install: ["corepack", "pnpm", "install", "--frozen-lockfile"],
    build: ["corepack", "pnpm", "build"],
  },
  {
    name: "wordle-plus",
    framework: "Svelte 3",
    repository: "https://github.com/MikhaD/wordle.git",
    revision: "199122be1f3ed71f5cf4abd5748debd91ee540a0",
    licence: "GPL-3.0",
    install: ["npm", "install"],
    build: ["npm", "run", "build"],
  },
  // The floor of the drop-in claim: the official starter templates, unedited.
  ...["react-ts", "vue-ts", "svelte-ts"].map(template => ({
    name: `vite-${template}`,
    framework: template,
    scaffold: ["npx", "--yes", "create-vite@9.1.2", `vite-${template}`, "--template", template],
    licence: "MIT (vitejs/vite)",
    install: ["npm", "install"],
    build: ["npm", "run", "build"],
  })),
];

const run = (cmd, cwd, env = process.env) =>
  Bun.spawnSync({ cmd, cwd, env, stdout: "pipe", stderr: "pipe" });
const output = result => `${result.stdout.toString()}${result.stderr.toString()}`;
const lastLine = text => text.split("\n").filter(line => line.trim()).at(-1) ?? "";

// Renders one staged application in its own process: a failed document load
// leaves the harness without an active document, and one fixture must not be
// able to poison the next one's result.
async function render(staging, frames, reportPath) {
  const native = createRequire(import.meta.url)(join(repository, "target/release/blitsen.node"));
  const report = { loaded: false, error: null, nodes: 0, elements: 0, colours: 0 };
  try {
    native.runDocumentScriptsHarness(join(staging, "index.html"), WIDTH, HEIGHT);
    report.loaded = true;
  } catch (error) {
    report.error = error.message;
  }
  if (report.loaded) {
    await Bun.sleep(SETTLE_MS);
    try {
      const snapshot = JSON.parse(native.snapshotDocumentHarness());
      const byHandle = new Map(snapshot.nodes.map(node => [node.handle, node]));
      const inBody = node => {
        for (let current = node; current; current = byHandle.get(current.parent)) {
          if (current.tag === "body") return current !== node;
        }
        return false;
      };
      report.nodes = snapshot.nodes.length;
      report.elements = snapshot.nodes.filter(inBody).length;
      report.colours = snapshot.paint_colors.length;
      const html = native.captureDocumentHarnessHtml()
        .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, "");
      await writeFile(join(staging, "capture.html"), `<!DOCTYPE html><html>${html}</html>`);
      native.recordDocumentAnimationHarness(
        join(staging, "capture.html"), "", frames, 1, WIDTH, HEIGHT);
    } catch (error) {
      report.error = error.message;
    }
  }
  await writeFile(reportPath, JSON.stringify(report));
}

if (process.argv[2] === "--render") {
  await render(process.argv[3], process.argv[4], process.argv[5]);
  process.exit(0);
}

// `blitsen build` stages the output before it embeds it, so the render evidence
// stages it the same way. The gate refuses these applications, so this is what
// they would look like if it did not.
async function stage(dist, staging) {
  const plan = await planIngest(dist);
  for (const file of plan.files) {
    const staged = join(staging, ...file.relative.split("/"));
    await mkdir(dirname(staged), { recursive: true });
    if ([".html", ".htm", ".css"].includes(extname(file.relative).toLowerCase())) {
      const resolutions = plan.resolutions.get(file.relative);
      await writeFile(staged, rewriteRootRelativeReferences(
        await readFile(file.absolute, "utf8"), file.relative,
        path => resolutions?.get(path) ?? null));
    } else await copyFile(file.absolute, staged);
  }
  return plan.files.length;
}

const options = { only: null, work: null, out: join(repository, "target/third-party") };
for (let index = 2; index < process.argv.length; index += 1) {
  const flag = process.argv[index].slice(2);
  if (!(flag in options)) throw new Error(`unknown option: ${process.argv[index]}`);
  options[flag] = process.argv[++index];
}

for (const command of ["git", "npm", "npx", "corepack"]) {
  if (!Bun.which(command)) throw new Error(`missing required command: ${command}`);
}

const addon = await buildAddon({ purpose: "third-party", release: true });

const work = options.work ?? await mkdtemp(join(tmpdir(), "blitsen-third-party-"));
await mkdir(work, { recursive: true });
await rm(options.out, { recursive: true, force: true });
await mkdir(options.out, { recursive: true });
const cli = join(repository, "packages/blitsen/bin/blitsen.mjs");
const environment = { ...process.env, BLITSEN_NATIVE_PATH: addon };
const results = [];

for (const fixture of FIXTURES) {
  if (options.only && options.only !== fixture.name) continue;
  const source = join(work, fixture.name);
  const result = { name: fixture.name, framework: fixture.framework, licence: fixture.licence };
  results.push(result);
  console.log(`\n=== ${fixture.name} (${fixture.framework})`);

  if (!await Bun.file(join(source, "package.json")).exists()) {
    await rm(source, { recursive: true, force: true });
    if (fixture.scaffold) {
      const scaffold = run(fixture.scaffold, work);
      if (scaffold.exitCode !== 0) throw new Error(`scaffold failed:\n${output(scaffold)}`);
    } else {
      await mkdir(source, { recursive: true });
      for (const command of [
        ["git", "init", "-q", "."],
        ["git", "remote", "add", "origin", fixture.repository],
        ["git", "fetch", "-q", "--depth", "1", "origin", fixture.revision],
        ["git", "checkout", "-q", "--detach", "FETCH_HEAD"],
      ]) {
        const step = run(command, source);
        if (step.exitCode !== 0) throw new Error(`${command.join(" ")} failed:\n${output(step)}`);
      }
    }
  }
  for (const command of [fixture.install, fixture.build]) {
    const step = run(command, source);
    if (step.exitCode !== 0) {
      result.status = `the application's own '${command.join(" ")}' failed`;
      break;
    }
  }
  if (result.status) continue;

  const dist = join(source, "dist");
  const doctor = run([process.execPath, cli, "doctor", dist, "--json"], source, environment);
  const report = JSON.parse(doctor.stdout.toString());
  result.doctor = { errors: report.errors, warnings: report.warnings };
  result.codes = [...new Set(report.diagnostics
    .filter(item => item.severity === "error").map(item => item.code))].sort();
  console.log(`doctor: ${report.errors} errors, ${report.warnings} warnings `
    + `(${result.codes.join(", ") || "none"})`);

  const exported = run(
    [process.execPath, cli, "build", dist, "--outfile", join(options.out, fixture.name)],
    source, environment);
  result.build = exported.exitCode === 0 ? "ok" : lastLine(exported.stderr.toString());
  console.log(`blitsen build: ${result.build}`);

  const staging = join(work, `${fixture.name}.staged`);
  await rm(staging, { recursive: true, force: true });
  result.assets = await stage(dist, staging);
  // Not `<name>` itself: a successful build writes the executable there, and the
  // directory then collides with it. Every build was refused when this was written.
  const frames = join(options.out, `${fixture.name}.frames`);
  await mkdir(frames, { recursive: true });
  const reportPath = join(frames, "render.json");
  const rendered = run(
    [process.execPath, import.meta.path, "--render", staging, frames, reportPath],
    repository, environment);
  if (!await Bun.file(reportPath).exists()) {
    throw new Error(`render process produced no report:\n${output(rendered)}`);
  }
  result.render = JSON.parse(await readFile(reportPath, "utf8"));
  // An exception thrown inside the application's own render pass — React catches
  // and rethrows it — reaches the host as an unhandled error rather than through
  // the harness call, so the reason a mount produced nothing is only in stderr.
  result.render.error ??= rendered.stderr.toString()
    .match(/^(?:[A-Za-z]*Error|error): .+$/m)?.[0] ?? null;
  result.rendered = result.render.loaded
    && result.render.elements >= MOUNTED_ELEMENTS
    && result.render.colours >= PAINTED_COLOURS;
  result.status = !result.render.loaded
    ? `document did not load: ${result.render.error}`
    : result.rendered
      ? `rendered ${result.render.elements} elements in ${result.render.colours} colours`
      : `blank: ${result.render.elements} elements below <body>, `
        + `${result.render.colours} painted colour(s)`
        + (result.render.error ? `, ${result.render.error}` : "");
  console.log(`render: ${result.status}`);
}

await writeFile(join(options.out, "summary.json"), JSON.stringify(results, null, 2));
console.log("\nThird-party adoption (unmodified builds):");
for (const result of results) {
  console.log(`  ${result.name.padEnd(16)} doctor ${String(result.doctor?.errors ?? "-").padStart(3)} errors`
    + `  build ${result.build === "ok" ? "ok" : "refused"}  ${result.status}`);
}
console.log(`Frames and summary.json: ${options.out}`);
if (!options.work) console.log(`Sources retained at ${work} (pass --work to reuse them).`);

const failed = results.filter(result => !result.rendered);
if (failed.length > 0) {
  console.log(`\n${failed.length}/${results.length} applications did not render unmodified. `
    + "M3b's adoption claim (P10) is not met; see docs/M3B.md.");
  process.exit(1);
}

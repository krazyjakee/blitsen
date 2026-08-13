// Typechecks the published definitions the way a user's editor will (issue #74).
//
// Opt-in and not in CI, for the reason `test:third-party` is: it needs npm and
// the network to fetch a TypeScript compiler this repository otherwise has no
// use for. The check that *is* in `bun test` is the drift check —
// `test/types.test.mjs`, which refuses definitions and runtime that disagree.
// This is the other half: that the definitions compile at all, that they accept
// the documented usage, and that they reject what they should.
//
//   bun run --cwd packages/blitsen test:types [--typescript <version>]
//
// The package is staged into a scratch project's `node_modules` rather than
// linked, so resolution goes through `package.json` exports exactly as it will
// for a user running `npm i -D blitsen`.
import { cp, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { repository } from "./build-addon.mjs";

const PACKAGE = join(repository, "packages/blitsen");
const FIXTURES = join(PACKAGE, "test/fixtures/types");
// The files the package publishes, which is what a user gets. Anything outside
// this list is repository furniture and must not be needed to typecheck.
const PUBLISHED = ["package.json", "index.d.ts", "index.mjs", "src", "bin", "tsconfig.json"];

const options = { typescript: "5.9.2" };
for (let index = 2; index < process.argv.length; index += 1) {
  const flag = process.argv[index].slice(2);
  if (!(flag in options)) throw new Error(`unknown option: ${process.argv[index]}`);
  options[flag] = process.argv[++index];
}

if (!Bun.which("npm")) throw new Error("missing required command: npm");

const work = await mkdtemp(join(tmpdir(), "blitsen-types-"));
await cp(FIXTURES, work, { recursive: true });
await writeFile(join(work, "package.json"),
  JSON.stringify({ name: "blitsen-types-check", private: true, type: "module" }, null, 2));
// Two projects, because the two halves are checked differently: one must
// compile clean, the other must produce no diagnostics *because* every error it
// does produce was expected by a `@ts-expect-error` directly above it.
for (const [name, file] of [["accepted", "accepted.ts"], ["rejected", "rejected.ts"]])
  await writeFile(join(work, `tsconfig.${name}.json`),
    JSON.stringify({ extends: "blitsen/tsconfig.json", include: [file] }, null, 2));

const run = (cmd, cwd = work) =>
  Bun.spawnSync({ cmd, cwd, stdout: "pipe", stderr: "pipe" });
const output = result => `${result.stdout.toString()}${result.stderr.toString()}`.trim();

console.log(`installing typescript@${options.typescript}`);
const install = run(["npm", "install", "--no-audit", "--no-fund", "--silent",
  `typescript@${options.typescript}`]);
if (install.exitCode !== 0) throw new Error(`could not install typescript:\n${output(install)}`);

// After the install, not before it: npm prunes anything in `node_modules` that
// no dependency asked for, and would take the staged package straight back out.
const staged = join(work, "node_modules/blitsen");
for (const entry of PUBLISHED)
  await cp(join(PACKAGE, entry), join(staged, entry), { recursive: true });

let failed = false;
for (const name of ["accepted", "rejected"]) {
  const check = run([join(work, "node_modules/.bin/tsc"), "-p", `tsconfig.${name}.json`]);
  const report = output(check);
  if (check.exitCode === 0) console.log(`${name}: ok`);
  else {
    failed = true;
    console.log(`${name}: FAILED\n${report}`);
  }
}

if (failed) {
  console.log(`\nProject retained at ${work}`);
  process.exitCode = 1;
} else {
  await rm(work, { recursive: true, force: true });
  console.log("\nThe published types compile, accept the documented usage, and reject misuse.");
}

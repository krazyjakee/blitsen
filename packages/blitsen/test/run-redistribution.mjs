// The exporter redistribution gate (issue #121).
//
//     bun run --cwd packages/blitsen test:licensing
//
// LICENSING.md: no `blitsen build` may claim redistribution compliance until an
// automated test extracts the embedded notices from a *built artifact* and finds
// them complete. This is that test, and it asks the artifact rather than the
// build: the executable prints what it carries, and what it carries is compared
// with the dependency graph `cargo` resolved for the runtime inside it.
//
// It also holds down the two ways the claim could quietly become false:
// packaging and signing must not remove the notices, and an export that has none
// must say so rather than pass silently.
import { strict as assert } from "node:assert";
import { cp, mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { buildAddon, capture, repository } from "./build-addon.mjs";
import { RUST_TARGETS } from "./generate-notices.mjs";
import { auditNotices, collectNotices, writeNotices } from "../src/notices.mjs";
import { hostTarget, packageVersion, resolvePhase2Runtime, TARGETS } from "../src/runtime.mjs";

const CLI = join(repository, "packages/blitsen/bin/blitsen.mjs");

const run = (command, args) => capture([command, ...args], { cwd: repository });

const addon = await buildAddon({ purpose: "the redistribution gate", release: true });
const runtime = await resolvePhase2Runtime();
// The gate links a platform package, and the notices table also has Android
// rows that no platform package carries — so the host has to be a desktop one.
const target = hostTarget();
if (!TARGETS.includes(target)) throw new Error(`no platform package to gate on ${target}`);

function cli(directory, args, environment = {}) {
  return capture([process.execPath, CLI, ...args], {
    cwd: directory,
    env: { ...process.env, BLITSEN_NATIVE_PATH: addon, ...environment },
  });
}

function licenses(executable) {
  const { code, stdout: text, stderr: error } = capture([executable, "--licenses"]);
  return { code, text, error };
}

const workspace = await mkdtemp(join(tmpdir(), "blitsen-licensing-"));
try {
  // What the runtime being linked actually depends on, asked of cargo now
  // rather than read from the file the build embedded — otherwise the test
  // would be comparing the notices with themselves.
  const collected = await collectNotices({
    target: RUST_TARGETS[target], root: "blitsen-runtime", run,
  });
  const problems = auditNotices(collected);
  assert.deepEqual(problems, [],
    `packages whose terms this export cannot honour:\n  ${problems.join("\n  ")}`);
  assert.ok(collected.packages.length > 100,
    `only ${collected.packages.length} packages resolved; the graph was not read`);

  // A release stages the notices into the platform package beside the runtime
  // it just built (`bun run notices`). A checkout has no platform package, so
  // the gate stages them the same way against the runtime it is about to link —
  // otherwise this would only ever run in CI.
  const staged = join(dirname(runtime.path), "NOTICES.txt");
  if (!await Bun.file(staged).exists()) {
    await writeNotices(dirname(runtime.path), collected, { version: await packageVersion() });
    console.log(`Staged notices beside the runtime under test: ${staged}`);
  }

  await cp(join(repository, "examples/pong"), join(workspace, "dist"), { recursive: true });
  const hook = join(workspace, "sign.mjs");
  // A stand-in for a real signing hook: it rewrites the artifact in place, which
  // is what codesign and signtool do and the thing that could strip a section.
  //
  // JavaScript rather than the `/bin/sh` script this was, because the hook runs
  // on the host and Windows has no `sh` on PATH. The argument is rejoined and
  // unwrapped because Windows runs the hook through `cmd /c`, which passes the
  // quotes around the path straight through to it (#134).
  await writeFile(hook, [
    'import { utimes } from "node:fs/promises";',
    'const artifact = process.argv.slice(2).join(" ").replace(/^"(.*)"$/s, "$1");',
    "const now = new Date();",
    "await utimes(artifact, now, now);",
    "",
  ].join("\n"));
  const sign = `${process.execPath} ${hook}`;

  const built = cli(workspace, ["build", "dist", "--out", "Notices"]);
  assert.equal(built.code, 0, `build failed:\n${built.stdout}\n${built.stderr}`);
  assert.match(built.stdout, /Third-party notices: embedded/,
    `the build did not report embedded notices:\n${built.stdout}`);
  assert.doesNotMatch(built.stdout, /not cleared for redistribution/,
    "an export that carries its notices must not still say it is uncleared");

  const executable = join(workspace, target.startsWith("win32-") ? "Notices.exe" : "Notices");
  const printed = licenses(executable);
  assert.equal(printed.code, 0, `the artifact could not print its notices: ${printed.error}`);

  // Completeness, package by package: every dependency in the graph, named with
  // the version that was linked.
  const missing = collected.packages.filter(entry =>
    !printed.text.includes(`${entry.name} ${entry.version}`));
  assert.deepEqual(missing.map(entry => `${entry.name} ${entry.version}`), [],
    `${missing.length} linked packages are absent from the embedded notices`);

  // The two the licensing document names: the engine, and the most demanding
  // term in the tree.
  assert.match(printed.text, /quickjs/i, "the JavaScript engine is not named in the notices");
  assert.match(printed.text, /\bstylo\b/i, "Stylo is not named in the notices");
  const mpl = collected.packages.filter(entry => /MPL-2\.0/i.test(entry.license ?? ""));
  assert.ok(mpl.length > 0, "no MPL-2.0 package resolved, which cannot be right for this tree");
  assert.match(printed.text, /SOURCE OFFER \(MPL-2\.0\)/,
    "MPL-2.0 packages are linked and no source offer travels with the artifact");
  for (const entry of mpl) {
    assert.ok(printed.text.includes(entry.source ?? entry.repository ?? ""),
      `${entry.name} is MPL-2.0 and its source is not named in the offer`);
  }

  // Every distinct licence text, in full. A list of package names without the
  // permission text is not what MIT asks to travel with the software.
  const texts = collected.licences.filter(licence =>
    !printed.text.includes(licence.text.split("\n").find(line => line.trim().length > 40) ?? ""));
  assert.deepEqual(texts.map(licence => licence.packages[0]), [],
    "some licence texts are named in the notices but not reproduced");

  // Packaging and signing rewrite the artifact. The claim has to survive that,
  // which is why it is asserted after `--sign` rather than before.
  const signed = cli(workspace, ["build", "dist", "--out", "Signed",
    "--app-version", "1.2.3", "--sign", sign]);
  assert.equal(signed.code, 0, `signed build failed:\n${signed.stdout}\n${signed.stderr}`);
  // macOS packaging moves the executable into a bundle — `Signed.app/Contents/
  // MacOS/Signed` — so a packaged build is not where an unpackaged one was, and
  // this is a packaged build: `--app-version` asks for platform artifacts.
  const signedExecutable = process.platform === "darwin"
    ? join(workspace, "Signed.app", "Contents", "MacOS", "Signed")
    : join(workspace, target.startsWith("win32-") ? "Signed.exe" : "Signed");
  const afterSigning = licenses(signedExecutable);
  assert.equal(afterSigning.code, 0,
    `a signed artifact could not print its notices: ${afterSigning.error}`);
  assert.equal(afterSigning.text, printed.text,
    "signing changed what the artifact reports as its third-party notices");

  // The other half of the gate: an export with no notices must fail loudly
  // rather than inherit the claim from the one that had them.
  const without = cli(workspace, ["build", "dist", "--out", "Bare"],
    { BLITSEN_NOTICES_PATH: join(workspace, "does-not-exist.txt") });
  assert.equal(without.code, 0, `build failed:\n${without.stderr}`);
  assert.match(without.stdout, /not cleared for redistribution/,
    "an export without notices claimed nothing about it");
  const bare = licenses(join(workspace, target.startsWith("win32-") ? "Bare.exe" : "Bare"));
  assert.notEqual(bare.code, 0, "an export without notices printed some anyway");
  assert.match(bare.error, /not cleared for redistribution/,
    `an artifact without notices should say why: ${bare.error}`);

  const bytes = (await stat(executable)).size;
  console.log(`Redistribution gate passed: ${collected.packages.length} linked packages, `
    + `${collected.licences.length} distinct licence texts, ${mpl.length} MPL-2.0 source offers.`);
  console.log(`  Embedded in a ${bytes.toLocaleString()}-byte artifact and read back from it, `
    + "before and after signing.");
  console.log(`  Runtime: ${runtime.path} (blitsen ${await packageVersion()})`);
} finally {
  await rm(workspace, { recursive: true, force: true });
}

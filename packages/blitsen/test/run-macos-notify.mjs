// Issue #253: the macOS notification identity, in development and packaged.
//
// Apple gates `UNUserNotificationCenter` on an application identity — a bundle
// identifier and a signature — and refuses a process that has none. Everything
// this checks is a property of the *process*, not of the code it runs, so the
// only honest way to check it is to ask real processes the same question: one
// launched bare the way `blitsen run` launches, one launched from the bundle
// `--dev-bundle` builds, and one launched from a bundle `blitsen build` writes.
// There is no in-process shortcut, which is why this is an acceptance runner
// rather than a unit test.
//
// What it deliberately does not do is submit a notification and wait for the
// user. Authorization is a person's decision and CI has no person; a runner
// that demanded a granted permission would be asserting on the runner's
// notification settings rather than on Blitsen. The line between the processes
// is whether the platform will talk to them at all.
//
//     bun run --cwd packages/blitsen test:macos-notify
import { strict as assert } from "node:assert";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { buildAddon, repository } from "./build-addon.mjs";
import { developmentBundle, developmentIdentifier, DEVELOPMENT_SIGNATURE, packageBuild,
  signArtifact } from "../src/packaging.mjs";
import { packageVersion } from "../src/runtime.mjs";

if (process.platform !== "darwin") {
  console.log(`macOS notification identity: not applicable on ${process.platform}`);
  process.exit(0);
}

const CLI = join(repository, "packages/blitsen/bin/blitsen.mjs");
const PROBE = join(import.meta.dir, "macos-notify-probe.mjs");
const NOTIFY_MODULE = join(import.meta.dir, "../src/native/notify.mjs");
// The identity a packaged application is given here. Neither Blitsen's
// development namespace nor an identifier belonging to anything installed on
// this machine — that is the whole point of the issue.
const PACKAGED_IDENTIFIER = "com.blitsen.test.notify-identity";
const PERMISSIONS = ["default", "denied", "granted"];

const addon = await buildAddon({ purpose: "macOS notification identity", release: true });
const version = await packageVersion();
const workspace = await realpath(await mkdtemp(join(tmpdir(), "blitsen-notify-identity-")));

// The document the probe loads to have the native namespace installed. It has
// nothing in it: what is being measured is the process, and the smallest real
// document is the least that can distract from that.
const entrypoint = join(workspace, "probe-app", "index.html");
await mkdir(dirname(entrypoint), { recursive: true });
await writeFile(entrypoint,
  `<!doctype html><html><head><meta charset="utf-8"><title>Notify identity</title></head>\n`
  + `<body></body></html>\n`);

function run(cmd, { env = {} } = {}) {
  const result = Bun.spawnSync({
    cmd,
    cwd: workspace,
    env: { ...process.env, BLITSEN_NATIVE_PATH: addon, ...env },
    stdout: "pipe",
    stderr: "pipe",
  });
  return {
    code: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  };
}

/** Launches the probe under `launcher` and returns the one line it prints. */
function identityOf(launcher, what) {
  const result = run([launcher, PROBE, addon, NOTIFY_MODULE, entrypoint]);
  assert.equal(result.code, 0,
    `${what} exited ${result.code}:\n${result.stdout}\n${result.stderr}`);
  const line = result.stdout.split("\n").find(text => text.startsWith("identity "));
  assert(line, `${what} printed no identity line:\n${result.stdout}\n${result.stderr}`);
  return JSON.parse(line.slice("identity ".length));
}

/** What has to hold of every process macOS is willing to talk to. */
function assertEligible(identity, what) {
  assert.equal(identity.error, undefined,
    `${what} must reach the notification centre, got: ${identity.error}`);
  assert(PERMISSIONS.includes(identity.permission),
    `${what} must report a permission, got ${JSON.stringify(identity.permission)}`);
  assert.equal(identity.standard, true,
    `the standard Notification facade belongs on ${what}`);
}

function assertSigned(bundle, what) {
  const verified = run(["codesign", "--verify", "--strict", bundle]);
  assert.equal(verified.code, 0, `codesign rejected ${what}:\n${verified.stderr}`);
}

// ① A bare development run. The refusal is the deliverable here: it has to say
// what is missing and name a command that supplies it, because a developer who
// only learns that notifications "do not work in development" has been told the
// symptom of Apple's rule rather than the way around it.
const bare = identityOf(process.execPath, "the unbundled development host");
assert.equal(bare.standard, false,
  "the standard Notification facade must stay absent where the process has no identity");
assert.equal(bare.permission, undefined,
  `an unbundled host must not report a permission, got ${JSON.stringify(bare.permission)}`);
assert.match(bare.error, /blitsen run --dev-bundle/,
  `the refusal must name the command that fixes it, got: ${bare.error}`);
assert.doesNotMatch(bare.error, /com\.apple\.|Terminal|Script Editor/,
  `the refusal must not name another application's identity, got: ${bare.error}`);
console.log(`unbundled: ${bare.error}`);

// ② The bundle `blitsen run --dev-bundle` re-executes into, built here by the
// function the CLI calls and signed by the same ad-hoc default. The
// re-execution itself is not driven from here — `blitsen run` opens a window
// and does not return — so what is checked is that the artifact it hands the
// development host carries an identity the platform accepts.
const name = "Blitsen Notify Check";
const developmentIdentity = developmentIdentifier(name);
const development = await developmentBundle({
  directory: join(workspace, "cache"),
  name,
  identifier: developmentIdentity,
  launcher: process.execPath,
  version,
});
assert.equal(development.rebuilt, true, "the first development bundle must be built");
assert.match(developmentIdentity, /^com\.blitsen\.dev\./,
  `the development identity must be Blitsen's own, got ${developmentIdentity}`);
assert.match(await readFile(join(development.bundle, "Contents/Info.plist"), "utf8"),
  new RegExp(`<key>CFBundleIdentifier</key>\\n  <string>${developmentIdentity}</string>`));
assertSigned(development.bundle, `the bundle \`${DEVELOPMENT_SIGNATURE}\` produced`);
const bundled = identityOf(development.executable, "a development host inside its own bundle");
assertEligible(bundled, "a development host inside its own bundle");
console.log(`development bundle: ${developmentIdentity} reports ${bundled.permission}`);

// ③ Packaged execution, through the packaging the exporter itself uses. The
// probe replaces the application only because an exported application's own
// notification surface needs a window session, which a headless runner has no
// way to drive; the bundle around it is written and signed by `packageBuild`
// and `signArtifact`, which is what `blitsen build --bundle-id --sign` calls.
const staged = join(workspace, "packaged", "NotifyProbe");
await mkdir(dirname(staged), { recursive: true });
await copyFile(process.execPath, staged);
const packaged = await packageBuild({
  platform: "darwin", executable: staged, title: "Notify Probe",
  identifier: PACKAGED_IDENTIFIER, version,
});
await signArtifact({ command: DEVELOPMENT_SIGNATURE, artifact: packaged.bundle });
assertSigned(packaged.bundle, "the packaged bundle");
const exported = identityOf(packaged.executable, "a packaged application");
assertEligible(exported, "a packaged application");
assert.notEqual(PACKAGED_IDENTIFIER, developmentIdentity,
  "a development host must not run under the identity an export carries");
console.log(`packaged bundle: ${PACKAGED_IDENTIFIER} reports ${exported.permission}`);

// The two eligible processes have to agree, because they differ only in which
// identity they carry: a development loop that could do something the export
// cannot would prove nothing about what ships.
assert.equal(bundled.standard, exported.standard,
  "development and packaged execution must expose the same notification surface");

// And the CLI still writes an eligible bundle for a real application, which is
// the semantics this issue must leave alone: an `.app` carrying the identifier
// the build was given.
//
// Built without the signing hook, and #256 is why. A Phase 2 export is the
// runtime with the payload appended past `__LINKEDIT`, which is a layout
// `codesign` rejects outright — so no macOS export this project has ever
// produced could be signed, and the first `blitsen build --sign` over a linked
// export is what found that out. Signing here is therefore blocked on a defect
// of the bundle format rather than on anything notification-shaped, and the
// identity this file is about is the Info.plist identifier, which is asserted
// either way. `assertSigned` returns when #256 changes the layout.
const built = run([process.execPath, CLI, "build", join(repository, "examples/pong"),
  "--out", join(workspace, "Pong"), "--bundle-id", PACKAGED_IDENTIFIER, "--force"]);
assert.equal(built.code, 0, `the packaged build failed:\n${built.stdout}\n${built.stderr}`);
const application = join(workspace, "Pong.app");
assert.match(await readFile(join(application, "Contents/Info.plist"), "utf8"),
  new RegExp(`<key>CFBundleIdentifier</key>\\n  <string>${PACKAGED_IDENTIFIER}</string>`));
console.warn("DEFERRED: not signing the export — blocked on #256 (payload appended past __LINKEDIT)");

await rm(workspace, { recursive: true, force: true });
console.log("macOS notification identity passed");

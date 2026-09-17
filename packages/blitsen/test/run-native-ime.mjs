// Moving focus between editable controls in a real native window.
//
// The platform IME is reconciled by `sync_ime` against the window's own state,
// and only a winit window has that state: `blitsen/test` substitutes an
// in-memory window, so it cannot see this path at all. winit-appkit
// 0.31.0-beta.2 ignored `ImeRequest::Disable` once IME had been enabled, so the
// second focused text control failed with `AlreadyEnabled`, `pumpWindow` threw
// and every macOS application exited. This opens a window and walks focus
// through the transitions that reconciliation distinguishes.
//
// macOS only for now: that is where the failure was, and the other hosts'
// windows have not been proven under their CI displays with an input method.
//
//     bun run --cwd packages/blitsen test:native-ime
import { strict as assert } from "node:assert";
import { join } from "node:path";

import { buildAddon, repository } from "./build-addon.mjs";

if (process.platform !== "darwin") {
  console.log(`native IME focus: not applicable on ${process.platform}`);
  process.exit(0);
}

const addon = await buildAddon({ purpose: "native IME focus", release: true });
const application = Bun.spawnSync({
  cmd: [process.execPath, join(repository, "packages/blitsen/bin/blitsen.mjs"),
    join(import.meta.dir, "fixtures/ime-focus"), "--width", "480", "--height", "320"],
  cwd: repository,
  env: { ...process.env, BLITSEN_NATIVE_PATH: addon },
  stdout: "pipe",
  stderr: "pipe",
  timeout: 60_000,
});
const stdout = application.stdout.toString();
const stderr = application.stderr.toString();
assert.equal(application.exitCode, 0, `exit ${application.exitCode}\n${stdout}\n${stderr}`);
assert.match(stdout, /ime focus transitions survived/, `${stdout}\n${stderr}`);
console.log("Native IME focus transitions verified");

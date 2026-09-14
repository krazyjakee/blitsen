// The runtime migration must work after assets and the addon are embedded.
import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildStandalone } from "../src/export.mjs";
import { launch } from "../src/test.mjs";
import { buildAddon } from "./build-addon.mjs";
import { writeBunServicesFixture } from "./bun-services-fixture.mjs";

const scratch = await mkdtemp(join(tmpdir(), "blitsen-packaged-bun-"));
let app;
try {
  const root = join(scratch, "app");
  await mkdir(root);
  await writeBunServicesFixture(root);
  const addon = await buildAddon({ purpose: "packaged Bun services" });
  const result = await buildStandalone({ root, width: 200, height: 100,
    title: "Bun services", outfile: join(scratch, "Services") }, addon);
  app = await launch(result.outfile, { timeout: 20_000,
    env: { BLITSEN_AUDIO_OFFLINE: "1" } });
  await app.waitFor(() => globalThis.bunWorkerResult || globalThis.bunWorkerError);
  assert.equal(await app.evaluate(() => globalThis.bunWorkerError ?? null), null);
  assert.deepEqual(await app.evaluate(() => globalThis.bunWorkerResult), {
    count: 2, total: 7, bytes: [1, 2, 3], bun: Bun.version, hasDocument: false,
  });
  await app.assert(() => globalThis.mainBunResult === 42 && globalThis.transferredLength === 0);
  console.log("Packaged Bun services passed: SQLite transaction/rollback, filesystem, worker, buffer transfer");
} finally {
  await app?.close();
  await rm(scratch, { recursive: true, force: true });
}

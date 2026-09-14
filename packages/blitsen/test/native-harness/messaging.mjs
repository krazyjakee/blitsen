import { strict as assert } from "node:assert";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { writeBunServicesFixture } from "../bun-services-fixture.mjs";
import { native } from "./addon.mjs";

const directory = await mkdtemp(join(tmpdir(), "blitsen-bun-worker-"));
try {
  await writeBunServicesFixture(directory);
  native.runDocumentScriptsHarness(join(directory, "index.html"), 200, 100);
  const deadline = performance.now() + 5000;
  while (!globalThis.bunWorkerResult && !globalThis.bunWorkerError && performance.now() < deadline) {
    globalThis.__blitsenAnimationFrameTick(performance.now());
    await Bun.sleep(5);
  }
  assert.equal(globalThis.bunWorkerError, undefined);
  assert.deepEqual(globalThis.bunWorkerResult, {
    count: 2, total: 7, bytes: [1, 2, 3], bun: Bun.version, hasDocument: false,
  }, "Bun worker runs transactions, rolls back and transfers buffers");
  assert.equal(globalThis.transferredLength, 0);
  assert.equal(globalThis.mainBunResult, 42);
  await globalThis.mainWrite;
  assert.equal(await readFile(join(directory, "main.txt"), "utf8"), "main");
  assert.equal(await readFile(join(directory, "worker.txt"), "utf8"), "worker");

  const channel = new MessageChannel();
  try {
    const message = new Promise(resolve => channel.port2.onmessage = event => resolve(event.data));
    channel.port1.postMessage({ nested: [1, 2] });
    assert.deepEqual(await message, { nested: [1, 2] });
  } finally { channel.port1.close(); channel.port2.close(); }
  assert.throws(() => structuredClone(() => {}), { name: "DataCloneError" });
} finally {
  globalThis.bunWorker?.terminate();
  globalThis.__blitsenDisposeContext?.();
  await rm(directory, { recursive: true, force: true });
}

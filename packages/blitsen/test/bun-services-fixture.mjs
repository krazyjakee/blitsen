import { writeFile } from "node:fs/promises";
import { join } from "node:path";

export async function writeBunServicesFixture(directory) {
  await writeFile(join(directory, "index.html"), '<script type="module" src="main.js"></script>');
  await writeFile(join(directory, "main.js"), `
    import { writeFile } from "node:fs/promises";
    import { Database } from "bun:sqlite";
    const database = new Database(":memory:");
    globalThis.mainBunResult = database.query("select 42 as answer").get().answer;
    database.close();
    globalThis.mainWrite = writeFile(new URL("main.txt", import.meta.url), "main");
    const bytes = new Uint8Array([1, 2, 3]);
    const worker = new Worker(new URL("worker.js", import.meta.url), { type: "module" });
    globalThis.bunWorker = worker;
    worker.onmessage = event => { globalThis.bunWorkerResult = event.data; worker.terminate(); };
    worker.onerror = event => { globalThis.bunWorkerError = event.message; worker.terminate(); };
    worker.postMessage(bytes.buffer, [bytes.buffer]);
    globalThis.transferredLength = bytes.buffer.byteLength;
  `);
  await writeFile(join(directory, "worker.js"), `
    import { Database } from "bun:sqlite";
    import { writeFile } from "node:fs/promises";
    const db = new Database(":memory:");
    db.run("create table entries (value integer)");
    const insert = db.prepare("insert into entries values (?)");
    onmessage = async event => {
      db.transaction(() => { insert.run(3); insert.run(4); })();
      try { db.transaction(() => { insert.run(99); throw new Error("rollback"); })(); } catch {}
      const row = db.query("select count(*) as count, sum(value) as total from entries").get();
      await writeFile(new URL("worker.txt", import.meta.url), "worker");
      postMessage({ ...row, bytes: [...new Uint8Array(event.data)],
        bun: Bun.version, hasDocument: typeof document !== "undefined" });
      db.close();
    };
  `);
}

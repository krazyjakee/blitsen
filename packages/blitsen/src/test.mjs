import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";

/** Launch a QuickJS packaged export (or a runtime with args: [directory]). */
export async function launch(exportPath, options = {}) {
  const timeout = options.timeout ?? 10_000;
  const child = spawn(resolve(exportPath), options.args ?? [], {
    cwd: options.cwd,
    env: { ...process.env, ...options.env, BLITSEN_TEST_MODE: "1",
      ...(options.width === undefined ? {} : { BLITSEN_TEST_WIDTH: String(options.width) }),
      ...(options.height === undefined ? {} : { BLITSEN_TEST_HEIGHT: String(options.height) }) },
    stdio: ["pipe", "pipe", "pipe"], windowsHide: true,
  });
  let sequence = 0, step = 0, closed = false, exited = false, stderr = "";
  const pending = new Map();
  let readyResolve, readyReject;
  const ready = new Promise((resolve, reject) => { readyResolve = resolve; readyReject = reject; });
  const exit = new Promise(resolve => child.once("close", () => { exited = true; resolve(); }));
  const fail = error => {
    readyReject(error);
    for (const entry of pending.values()) { clearTimeout(entry.timer); entry.reject(error); }
    pending.clear();
  };
  child.on("error", fail);
  child.stdin.on("error", fail);
  child.stderr.on("data", data => { stderr = (stderr + data).slice(-32_000); options.onLog?.(String(data)); });
  child.on("close", (code, signal) => fail(new Error(`Application test process exited (${signal ?? code})\n${stderr}`)));
  const lines = createInterface({ input: child.stdout });
  lines.on("line", line => {
    if (!line.startsWith("BLITSEN_TEST:")) { options.onLog?.(line); return; }
    let message;
    try { message = JSON.parse(line.slice("BLITSEN_TEST:".length)); }
    catch { fail(new Error("Invalid application test response")); return; }
    if (message.ready) { readyResolve(message); return; }
    const entry = pending.get(message.id);
    if (!entry) return;
    pending.delete(message.id);
    clearTimeout(entry.timer);
    if (message.error) entry.reject(new Error(message.error));
    else entry.resolve(message.result);
  });
  const startupTimer = setTimeout(() => readyReject(new Error(
    `Application test launch timed out. Use a QuickJS export built with application UI test support.\n${stderr}`)), timeout);
  let viewport;
  try { viewport = await ready; }
  catch (error) { child.kill(); await exit; throw error; }
  finally { clearTimeout(startupTimer); }

  const request = (command, arguments_ = {}) => new Promise((resolve, reject) => {
    if (closed || exited) { reject(new Error("Application test session is closed")); return; }
    const id = ++sequence;
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Application test ${command} timed out\n${stderr}`));
      // A timed-out command may still mutate the application later. Terminate
      // the process so subsequent steps cannot silently race that command.
      child.kill();
    }, timeout + (command === "settle" ? arguments_.ms ?? 0 : 0));
    pending.set(id, { resolve, reject, timer });
    child.stdin.write(JSON.stringify({ ...arguments_, id, command }) + "\n", error => {
      if (error) { clearTimeout(timer); pending.delete(id); reject(error); }
    });
  });
  const screenshot = async path => {
    const png = Buffer.from(await request("screenshot"), "base64");
    if (path) await writeFile(path, png);
    return png;
  };
  const evidence = async (label, operation) => {
    const number = ++step;
    try {
      const result = await operation();
      if (options.screenshots === "steps" && options.artifactsDir) {
        await mkdir(options.artifactsDir, { recursive: true });
        await screenshot(join(options.artifactsDir, `${number}-${label}.png`));
      }
      return result;
    } catch (error) {
      try {
        error.events = await request("eventLog");
        error.screenshot = await screenshot();
        if (options.artifactsDir) {
          await mkdir(options.artifactsDir, { recursive: true });
          const prefix = join(options.artifactsDir, `${number}-${label}-failed`);
          await writeFile(`${prefix}.png`, error.screenshot);
          await writeFile(`${prefix}.json`, JSON.stringify({ error: error.message, events: error.events }, null, 2));
          error.artifacts = prefix;
        }
      } catch (captureError) { error.evidenceError = captureError.message; }
      throw error;
    }
  };
  const query = locator => request("query", { locator });
  const settle = (ms = 50) => request("settle", { ms });
  const pointer = async (type, { x, y, target, ...init }) => {
    await request("pointer", { type, x, y, target, init });
    await settle(0);
  };
  const expression = value => typeof value === "function" ? `(${value.toString()})()` : String(value);
  const evaluate = script => request("evaluate", { expression: expression(script) });
  const predicate = script => `(() => { const result = (${expression(script)});
    if (result && typeof result.then === 'function')
      throw new TypeError('test predicates must be synchronous; use waitFor for asynchronous work');
    return Boolean(result); })()`;
  const app = {
    viewport, query, evaluate, settle, screenshot,
    eventLog: () => request("eventLog"),
    pointer: (type, init) => evidence(type, () => pointer(type, init)),
    wheel: (init) => evidence("wheel", () => pointer("wheel", init)),
    click: locator => evidence("click", async () => {
      let point;
      if (typeof locator === "object" && "x" in locator) point = locator;
      else {
        const found = await query(locator);
        if (found.length !== 1) throw new Error(`Click expected one element, found ${found.length}`);
        const element = found[0], box = element.box;
        if (element.disabled) throw new Error("Element is disabled");
        if (box.width <= 0 || box.height <= 0) throw new Error("Element has no visible layout box");
        const x = box.x + box.width / 2, y = box.y + box.height / 2;
        if (x < 0 || y < 0 || x >= element.viewport.width || y >= element.viewport.height)
          throw new Error("Element is outside the viewport; scroll before clicking");
        point = { x, y, target: element.handle };
      }
      await pointer("pointermove", point);
      await pointer("pointerdown", { ...point, button: 0 });
      await pointer("pointerup", { ...point, button: 0 });
      await settle();
    }),
    key: (type, init) => evidence(type, async () => {
      await request("key", { type, init });
      await settle(0);
    }),
    type: text => evidence("type", async () => {
      await request("text", { text });
      await settle();
    }),
    assert: script => evidence("assert", async () => {
      if (!await evaluate(predicate(script))) throw new Error(`Application assertion failed: ${expression(script)}`);
    }),
    waitFor: (script, { timeout: waitTimeout = timeout } = {}) => evidence("waitFor", async () => {
      const deadline = Date.now() + waitTimeout;
      do {
        if (await evaluate(predicate(script))) return;
        await settle(20);
      } while (Date.now() < deadline);
      throw new Error(`Application condition timed out: ${expression(script)}`);
    }),
    close: async () => {
      if (closed) return;
      try { if (!exited) await request("close"); }
      finally {
        closed = true;
        child.stdin.end();
        const timer = setTimeout(() => child.kill(), 1000);
        await exit;
        clearTimeout(timer);
        lines.close();
      }
    },
  };
  return app;
}

export default Object.freeze({ launch });

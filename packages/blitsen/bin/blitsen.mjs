#!/usr/bin/env node

import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { main } from "../src/cli.mjs";
import { buildStandalone } from "../src/export.mjs";

let runtime = null;
try {
  const require = createRequire(import.meta.url);
  const configuredPath = process.env.BLITSEN_NATIVE_PATH;
  const nativePath = configuredPath?.startsWith("file:")
    ? fileURLToPath(configuredPath)
    : configuredPath ?? fileURLToPath(new URL("../native/blitsen.node", import.meta.url));
  const native = require(nativePath);
  const engine = new native.Engine();
  runtime = {
    openDirectory(options) {
      return engine.openDirectory(options);
    },
    reloadCSS: engine.reloadCSS ? file => engine.reloadCSS(file) : null,
    reloadDirectory: engine.reloadDirectory ? () => engine.reloadDirectory() : null,
    pumpWindow: engine.pumpWindow ? () => engine.pumpWindow() : null,
    waitForNextFrame: delay => Bun.sleep(delay),
    build: options => buildStandalone(options, nativePath),
  };
} catch {}

const code = await main(process.argv.slice(2), console, runtime);
// The window is the application: when it closes, the run is over. Returning
// here instead would hand control back to Bun's event loop, which is not the
// document's — every interval, animation callback and worker message the
// application left behind is still queued on it, and draining them runs an
// application whose window, renderer and document have already been dropped.
// An application that armed a single `setInterval` also kept the process alive
// with nothing on screen. Bun flushes both streams through `process.exit`, so
// nothing already written is lost.
process.exit(code);

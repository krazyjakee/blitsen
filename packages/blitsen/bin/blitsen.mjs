#!/usr/bin/env node

import { fileURLToPath } from "node:url";
import { main } from "../src/cli.mjs";
import { buildStandalone } from "../src/export.mjs";
import { openRuntime } from "../src/runtime.mjs";

let runtime = null;
try {
  const configuredPath = process.env.BLITSEN_NATIVE_PATH;
  const nativePath = configuredPath?.startsWith("file:")
    ? fileURLToPath(configuredPath)
    : configuredPath ?? fileURLToPath(new URL("../native/blitsen.node", import.meta.url));
  const resolved = { path: nativePath };
  runtime = {
    ...openRuntime(resolved, { waitForNextFrame: delay => Bun.sleep(delay) }),
    build: options => buildStandalone(options, resolved),
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

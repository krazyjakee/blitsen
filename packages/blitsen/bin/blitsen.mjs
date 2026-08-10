#!/usr/bin/env node

import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { main } from "../src/cli.mjs";

let runtime = null;
try {
  const require = createRequire(import.meta.url);
  const configuredPath = process.env.BLITSEN_NATIVE_PATH;
  const nativePath = configuredPath?.startsWith("file:")
    ? fileURLToPath(configuredPath)
    : configuredPath ?? fileURLToPath(new URL("../native/blitsen.node", import.meta.url));
  const native = require(nativePath);
  runtime = {
    openDirectory(options) {
      const engine = new native.Engine();
      return engine.openDirectory?.(options) ?? engine.loadHTML(options.entrypoint);
    },
  };
} catch {}

process.exitCode = await main(process.argv.slice(2), console, runtime);

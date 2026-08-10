#!/usr/bin/env node

import { main } from "../src/cli.mjs";

let runtime = null;
try {
  const native = await import(process.env.BLITSEN_NATIVE_PATH ?? "../native/blitsen.node");
  runtime = {
    openDirectory(options) {
      const engine = new native.Engine();
      return engine.openDirectory?.(options) ?? engine.loadHTML(options.entrypoint);
    },
  };
} catch {}

process.exitCode = await main(process.argv.slice(2), console, runtime);

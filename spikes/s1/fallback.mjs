import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const native = require("./s1_winit_pump.node");
const samples = Number(process.env.S1_SAMPLES ?? 600);
const periodMicros = Number(process.env.S1_PERIOD_MICROS ?? 16667);
const busyMs = Number(process.env.S1_BUSY_MS ?? 0);

native.startFallback(samples, periodMicros);

const timer = setInterval(() => {
  const busyUntil = performance.now() + busyMs;
  while (performance.now() < busyUntil) {
    // Simulate JS/DOM work before the frame reaches the native host.
  }
  if (native.pumpWinit()) {
    clearInterval(timer);
    const stats = JSON.parse(native.fallbackStats());
    stats.simulated_js_work_ms = busyMs;
    console.log(JSON.stringify(stats, null, 2));
  }
}, periodMicros / 1000);

setTimeout(() => {
  clearInterval(timer);
  throw new Error("S1 fallback benchmark timed out");
}, Math.ceil((samples * periodMicros) / 1000) + 10_000).unref();

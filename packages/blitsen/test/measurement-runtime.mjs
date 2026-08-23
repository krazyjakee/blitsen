// A measurement must name the runtime it weighed. Runtime resolution normally
// prefers an installed platform package, which is correct for an export and
// dangerously ambiguous for a checkout benchmark: it can silently measure the
// previous release. Require the caller to pin the executable explicitly.
import { resolvePhase2Runtime } from "../src/runtime.mjs";

export async function pinnedPhase2Runtime({
  env = process.env,
  resolve = resolvePhase2Runtime,
} = {}) {
  if (!env.BLITSEN_RUNTIME_PATH) {
    throw new Error("measurement requires BLITSEN_RUNTIME_PATH naming this checkout's freshly "
      + "built Phase 2 runtime; run `cargo build --release -p blitsen-runtime` and set the path");
  }
  const runtime = await resolve({ env });
  if (runtime.source !== "environment") {
    throw new Error(`measurement runtime was resolved from ${runtime.source}, not BLITSEN_RUNTIME_PATH`);
  }
  return runtime;
}

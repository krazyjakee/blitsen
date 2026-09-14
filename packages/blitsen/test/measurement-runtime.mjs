// A measurement must name the runtime it weighed. Runtime resolution normally
// prefers an installed platform package, which is correct for an export and
// dangerously ambiguous for a checkout benchmark: it can silently measure the
// previous release. Require the caller to pin the executable explicitly.
import { resolveInterpreter } from "../src/runtime.mjs";

export async function pinnedInterpreter({
  env = process.env,
  resolve = resolveInterpreter,
} = {}) {
  if (!env.BLITSEN_RUNTIME_PATH) {
    throw new Error("measurement requires BLITSEN_RUNTIME_PATH naming this checkout's freshly "
      + "built Bun interpreter; run `bun scripts/build-bun-runtime.mjs` and set the path");
  }
  const runtime = await resolve({ env });
  if (runtime.source !== "environment") {
    throw new Error(`measurement runtime was resolved from ${runtime.source}, not BLITSEN_RUNTIME_PATH`);
  }
  return runtime;
}

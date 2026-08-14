// Stands in for codesign/signtool: records the artifact the hook was handed.
//
// JavaScript rather than the `/bin/sh` script this replaced, because the hook
// runs wherever the tests do and Windows has no `sh` on PATH — the sign step
// failed there with exit code 127, which is not a signing result (#134).
import { writeFile } from "node:fs/promises";

// On Windows the hook runs through `cmd /c`, and the quotes Blitsen puts around
// the path to survive spaces reach the interpreter intact: a native signing
// tool's C runtime both groups on them and strips them, Bun's argv does
// neither, so a path with a space arrives as several arguments with a quote at
// each end of the run. A signing hook written in JavaScript has to rejoin and
// unwrap them itself (#134).
const artifact = process.argv.slice(2).join(" ").replace(/^"(.*)"$/s, "$1");
if (!artifact) {
  console.error("usage: record-artifact.mjs <artifact>");
  process.exit(1);
}
await writeFile(`${artifact}.signed`, `${artifact}\n`);

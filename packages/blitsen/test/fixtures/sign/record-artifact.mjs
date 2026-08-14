// Stands in for codesign/signtool: records the artifact the hook was handed.
//
// JavaScript rather than the `/bin/sh` script this replaced, because the hook
// runs wherever the tests do and Windows has no `sh` on PATH — the sign step
// failed there with exit code 127, which is not a signing result (#134).
import { writeFile } from "node:fs/promises";

// On Windows the hook runs through `cmd /c`, so the quotes Blitsen puts around
// the path to survive spaces are still there when the interpreter parses its
// own arguments: a native signing tool's C runtime strips them, Bun's argv does
// not. A signing hook written in JavaScript therefore has to (#134).
const artifact = (process.argv[2] ?? "").replace(/^"(.*)"$/s, "$1");
if (!artifact) {
  console.error("usage: record-artifact.mjs <artifact>");
  process.exit(1);
}
await writeFile(`${artifact}.signed`, `${artifact}\n`);

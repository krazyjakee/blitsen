// Stands in for codesign/signtool: records the artifact the hook was handed.
//
// JavaScript rather than the `/bin/sh` script this replaced, because the hook
// runs wherever the tests do and Windows has no `sh` on PATH — the sign step
// failed there with exit code 127, which is not a signing result (#134).
import { writeFile } from "node:fs/promises";

const artifact = process.argv[2];
if (!artifact) {
  console.error("usage: record-artifact.mjs <artifact>");
  process.exit(1);
}
await writeFile(`${artifact}.signed`, `${artifact}\n`);

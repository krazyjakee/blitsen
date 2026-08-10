import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const native = require("./option2.node");
console.log(`uv_run result: ${native.pumpUv()}`);

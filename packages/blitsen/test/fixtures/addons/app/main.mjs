// Bun refuses `import greet from "./greet.node"` — Node-API addons load through
// require, not the ESM graph — so a module script reaches its addon this way.
// import.meta.url is the addon's neighbour wherever the export materialized it.
import { createRequire } from "node:module";

const greet = createRequire(import.meta.url)("./greet.node");
document.getElementById("greeting").textContent = greet.greeting;

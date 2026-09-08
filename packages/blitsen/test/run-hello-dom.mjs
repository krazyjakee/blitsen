import { runExample } from "./build-addon.mjs";

await runExample({
  purpose: "hello-dom", example: "hello-dom",
  hints: ["Expect a native window with a green panel reading ‘hi’; resize it, then close it."],
});

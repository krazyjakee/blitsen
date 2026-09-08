import { runExample } from "./build-addon.mjs";

await runExample({
  purpose: "interactive demo", example: "interactive", width: 960, height: 640,
  title: "Blitsen Interactive",
  hints: ["Interactive: click the control to expand it, then use ← → or Space."],
});

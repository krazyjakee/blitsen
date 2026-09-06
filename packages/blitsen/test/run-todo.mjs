import { runExample } from "./build-addon.mjs";

await runExample({
  purpose: "todo demo", example: "todo", width: 980, height: 760, title: "Blitsen Tasks",
  hints: [
    "Todo example: a persistent task list with priorities, search and filters.",
    "Tasks are saved locally between launches. Resize the app and use the custom",
    "borderless window controls to exercise its responsive desktop layout.",
  ],
});

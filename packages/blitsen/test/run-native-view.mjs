import { runExample } from "./build-addon.mjs";

await runExample({
  purpose: "native-view demo", example: "native-view", width: 700, height: 500, title: "blitsen-view",
  hints: [
    "blitsen-view: an application-drawn surface composited into the DOM frame.",
    "Expect an animating gradient with its own rounded corners, the red DOM underlay",
    "showing around and behind it, and a DOM chip drawn on top of it.",
  ],
});

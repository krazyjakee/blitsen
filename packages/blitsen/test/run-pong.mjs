import { runExample } from "./build-addon.mjs";

await runExample({
  purpose: "Pong", example: "pong", width: 720, height: 520, title: "Blitsen Pong",
  hints: ["Pong: W/S versus ↑/↓, Space serves or pauses. First player to 7 wins."],
});

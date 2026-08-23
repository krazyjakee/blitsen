// The bare application P1's size figure is written against, in one place.
//
// An HTML file that renders and nothing else. Anything larger measures the
// application rather than the runtime, and two size scripts measuring two
// different "bare" applications would produce two numbers nobody could put
// beside each other. `run-phase2-size.mjs` (desktop, issue #89) and
// `run-android-size.mjs` (Android, issue #150) both import this, which is what
// makes the desktop executable and the APK comparable at all.
// Electron and Tauri use the file itself. Blitsen and Android import its bytes,
// so every comparison packages the exact same document rather than three
// fixtures that merely look similar.
import { readFileSync } from "node:fs";

export const BARE_APP = readFileSync(
  new URL("./fixtures/size-comparison/web/index.html", import.meta.url), "utf8");

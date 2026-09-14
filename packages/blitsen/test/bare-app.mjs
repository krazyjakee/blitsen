// One bare HTML fixture for Bun, Electron and Tauri size measurements.
import { readFileSync } from "node:fs";

export const BARE_APP = readFileSync(
  new URL("./fixtures/size-comparison/web/index.html", import.meta.url), "utf8");

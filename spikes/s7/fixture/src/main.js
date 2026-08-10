import { add } from "./math.js";
import { redirected } from "./redirected.js";
import assetUrl from "../pixel.svg?url";

export async function run() {
  const { lazy } = await import("./lazy.js");
  return {
    total: add(20, 22),
    lazy,
    redirected,
    assetUrl,
    moduleUrl: import.meta.url,
  };
}

// What one process can say about its own macOS notification identity.
//
// Run by `run-macos-notify.mjs` three times over — bare, inside a development
// bundle, and from a packaged one — because the answer is a property of the
// process rather than of the code, and the only way to compare them is to ask
// the same question from each.
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

const [addonPath, modulePath, entrypoint] = process.argv.slice(2);
if (!addonPath || !modulePath || !entrypoint) {
  console.error("usage: macos-notify-probe.mjs <addon.node> <notify.mjs> <index.html>");
  process.exit(1);
}

// Loading the addon is not enough: the native namespace is installed when a
// document is loaded, so the probe loads one — the same headless path every
// other acceptance runner drives, with no window and no display.
const native = createRequire(import.meta.url)(addonPath);
native.runDocumentScriptsHarness(entrypoint, 320, 240);
const notify = (await import(pathToFileURL(modulePath).href)).default;

// The standard facade is the feature-detectable half of the same fact: the host
// installs `Notification` only where close is addressable, which on macOS means
// only where the process has a bundle identity.
const report = { standard: "Notification" in globalThis };
try {
  report.permission = await notify.permission();
} catch (error) {
  report.error = error.message;
}
console.log(`identity ${JSON.stringify(report)}`);

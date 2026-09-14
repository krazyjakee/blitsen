import { strict as assert } from "node:assert";
import { native } from "./addon.mjs";

const expected = Intl.NumberFormat("de-DE", { style: "currency", currency: "EUR" }).format(1234.5);
native.runBridgeHarness("<p></p>", `
  globalThis.intlResult = {
    currency: Intl.NumberFormat("de-DE", { style: "currency", currency: "EUR" }).format(1234.5),
    parts: Intl.DateTimeFormat("en-GB").formatToParts(new Date(0)).length,
    segments: [...new Intl.Segmenter("en", { granularity: "word" }).segment("hello world")].length,
    locale: Intl.DateTimeFormat().resolvedOptions().locale,
  };
`);
assert.equal(intlResult.currency, expected, "document retains Bun's Intl implementation");
assert(intlResult.parts > 1 && intlResult.segments > 1, "full Intl remains available");

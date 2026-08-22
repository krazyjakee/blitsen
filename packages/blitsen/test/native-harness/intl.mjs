// `Intl` (issue #237), through the bridge rather than through the formatters.
//
// The formatting itself is gated in Rust, against CLDR — see
// `dom_bridge/intl/mod.rs`. What is gated here is the half that only exists in
// JavaScript: that the object is installed at all, that the three prototype
// methods the engine ships in locale-blind form have been replaced, that the
// values crossing the boundary survive the trip, and that what is absent is
// absent rather than present and wrong.
import { strict as assert } from "node:assert";

import { native } from "./addon.mjs";

const results = JSON.parse(native.runBridgeHarness(
  `<div id="intl"></div>`,
  `{ const out = globalThis.__blitsenIntl = {};
     if (!("Intl" in globalThis)) throw new Error("Intl must be installed");

     out.decimal = new Intl.NumberFormat("de-DE").format(1234567.891);
     out.currency = new Intl.NumberFormat("en-US",
       { style: "currency", currency: "JPY" }).format(1234.5);
     out.percent = new Intl.NumberFormat("en-US", { style: "percent" }).format(0.256);
     out.compact = new Intl.NumberFormat("en-US", { notation: "compact" }).format(1234567);
     // A number JavaScript prints in exponential notation still reaches the
     // decimal parser as digits.
     out.huge = new Intl.NumberFormat("en-US").format(1e21);
     out.tiny = new Intl.NumberFormat("en-US", { maximumFractionDigits: 8 }).format(1e-7);

     const resolved = new Intl.NumberFormat("en-US",
       { style: "currency", currency: "JPY" }).resolvedOptions();
     out.resolved = [resolved.locale, resolved.style, resolved.currency,
       resolved.maximumFractionDigits];

     // The instant of the issue's own case: a session that opens at 09:30 in
     // New York, read from a UTC timestamp.
     const open = Date.UTC(2026, 6, 15, 13, 30);
     out.zoned = new Intl.DateTimeFormat("en-US",
       { timeZone: "America/New_York", dateStyle: "medium", timeStyle: "short" }).format(open);
     out.utc = new Intl.DateTimeFormat("en-US",
       { timeZone: "UTC", dateStyle: "medium", timeStyle: "short" }).format(open);
     out.defaultZone = new Intl.DateTimeFormat().resolvedOptions().timeZone;

     out.relative = new Intl.RelativeTimeFormat("en-US", { numeric: "auto" }).format(-1, "day");
     out.plural = ["one", "other"].map((_, index) =>
       new Intl.PluralRules("en-US").select(index + 1));
     out.list = new Intl.ListFormat("en-US", { type: "disjunction" }).format(["a", "b", "c"]);
     out.sorted = ["ö", "z", "a"].sort(new Intl.Collator("sv-SE").compare);

     // The three prototype methods. The engine's own versions ignore the locale
     // and return the invariant form, which is the silent-wrong-output failure
     // the manifest used to warn about.
     out.numberToLocale = (1234.5).toLocaleString("de-DE");
     out.dateToLocale = new Date(open).toLocaleDateString("en-GB", { timeZone: "UTC" });
     out.timeToLocale = new Date(open).toLocaleTimeString("en-GB",
       { timeZone: "UTC", hour: "2-digit", minute: "2-digit" });
     out.localeCompare = ["ä".localeCompare("z", "sv-SE"), "ä".localeCompare("z", "de-DE")];
     out.invalidDate = new Date(NaN).toLocaleDateString("en-US");

     // Absent is absent: a feature test has to be able to tell.
     out.absent = ["Segmenter", "DisplayNames", "DurationFormat", "supportedValuesOf"]
       .map(name => name in Intl);
     out.absentParts = "formatToParts" in Intl.NumberFormat.prototype;

     // A tag no parser accepts is a RangeError, as it is in a browser.
     try { new Intl.NumberFormat("this is not a tag"); out.badTag = null; }
     catch (error) { out.badTag = error.constructor.name; }
     // A zone the database does not have is refused rather than silently UTC.
     try { new Intl.DateTimeFormat("en", { timeZone: "Mars/Olympus" }); out.badZone = null; }
     catch (error) { out.badZone = String(error.message).includes("time zone"); }

     document.getElementById("intl").setAttribute("data-intl", "ok"); }`,
  320, 180));
assert.equal(
  results.nodes.find(node => node.attributes.id === "intl").attributes["data-intl"], "ok");

const out = globalThis.__blitsenIntl;

assert.equal(out.decimal, "1.234.567,891", "German separators, not the invariant form");
assert.equal(out.currency, "¥1,235", "the yen has no minor unit to show two digits of");
assert.equal(out.percent, "26%");
assert.equal(out.compact, "1.2M");
assert.equal(out.huge, "1,000,000,000,000,000,000,000",
  "a number printed as 1e21 in JavaScript still formats as digits");
assert.equal(out.tiny, "0.0000001");
assert.deepEqual(out.resolved, ["en-US", "currency", "JPY", 0],
  "resolvedOptions reports the digits that were used, which the currency decided");

assert.equal(out.zoned, "Jul 15, 2026, 9:30\u202fAM",
  "13:30 UTC is 09:30 in New York in July, which needs the zone's DST rules");
assert.equal(out.utc, "Jul 15, 2026, 1:30\u202fPM");
assert.match(out.defaultZone, /^[A-Za-z]+(\/[A-Za-z0-9_+-]+)*$/,
  `the default time zone is a real zone name: ${out.defaultZone}`);

assert.equal(out.relative, "yesterday");
assert.deepEqual(out.plural, ["one", "other"]);
assert.equal(out.list, "a, b, or c");
assert.deepEqual(out.sorted, ["a", "z", "ö"],
  "Swedish sorts o-umlaut after z, and a bound compare drops straight into sort");

assert.equal(out.numberToLocale, "1.234,5",
  "Number.prototype.toLocaleString honours the locale it is given");
assert.equal(out.dateToLocale, "15 Jul 2026");
assert.equal(out.timeToLocale, "13:30");
assert.deepEqual(out.localeCompare, [1, -1],
  "localeCompare orders by the locale's table rather than by code unit");
assert.equal(out.invalidDate, "Invalid Date");

assert.deepEqual(out.absent, [false, false, false, false],
  "what is not implemented is not present, so feature detection selects a fallback");
assert.equal(out.absentParts, false);
assert.equal(out.badTag, "RangeError");
assert.equal(out.badZone, true);

delete globalThis.__blitsenIntl;

// `os.locale()` (#237): the two values an application hands to the formatters.
// Reached through the published module the way `native-modules.mjs` reaches the
// rest of `blitsen/os`, because that is how an application reaches it.
const { default: nativeOs } = await import("../../src/native/os.mjs");
const locale = nativeOs.locale?.();
assert.ok(locale, "os.locale() is installed now that there is an Intl behind it");
assert.match(locale.language, /^[A-Za-z]{2,8}(-[A-Za-z0-9]{2,8})*$/,
  `os.locale().language is a language tag: ${locale.language}`);
assert.equal(locale.timeZone,
  new Intl.DateTimeFormat().resolvedOptions().timeZone,
  "the zone os.locale() reports is the one the formatters default to");

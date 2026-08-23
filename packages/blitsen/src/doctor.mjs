import { readFile } from "node:fs/promises";
import { extname } from "node:path";
import { loadApiManifest } from "./api-manifest.mjs";
import { HTML_EXTENSIONS, SCANNABLE_EXTENSIONS, walkFiles } from "./files.mjs";
import { absentNativeModules, platformOf } from "./native-modules.mjs";
import { HID_ACCESS } from "./packaging.mjs";
import { hostTarget } from "./runtime.mjs";

// Every rule below comes from the generated manifest, so `doctor` and the
// runtime cannot describe the same API differently. See COMPATIBILITY.md.
async function compatibilityRules() {
  const manifest = await loadApiManifest();
  const absent = new Map();
  for (const entry of manifest.apis) {
    if (entry.status !== "absent" || !entry.pattern) continue;
    absent.set(entry.code, [...(absent.get(entry.code) ?? []), entry.pattern]);
  }
  const javascript = manifest.usage.map(rule =>
    [rule.code, rule.severity, new RegExp(rule.pattern, "g"), rule.message, rule.guidance]);
  for (const [code, patterns] of absent) {
    const rule = manifest.diagnostics[code];
    javascript.push([code, rule.severity, new RegExp([...patterns, rule.extra].filter(Boolean)
      .join("|"), "g"), rule.message, rule.guidance]);
  }
  const bySource = kind => [...manifest.renderer, ...manifest.assets]
    .filter(rule => rule.kind === kind).map(rule =>
      [rule.code, rule.severity, new RegExp(rule.pattern, "gi"), rule.message, rule.guidance]);
  return { javascript, css: bySource("css"), html: bySource("html") };
}

let loaded;
const compatibility = () => (loaded ??= compatibilityRules());

// A `native:` module the target being built for does not have (#147).
//
// Not from the manifest, and the reason is worth stating: the manifest is
// generated from one bootstrap script shared by every build, so it can say a
// module has no members anywhere but not that a module exists on Linux and not
// on Android — that is decided by `cfg` in the Rust, which nothing on this side
// can read. `native-modules.mjs` carries the table and the reasoning.
//
// A rule per module rather than one rule with a capture, because the whole value
// of the finding is the sentence that says why the module is not there, and that
// sentence is different for every row.
//
// A warning, on the same argument as every other diagnostic here: an absent
// module's members are `undefined` rather than throwing, so
// `if (clipboard.writeText)` selects a fallback and the application survives.
// What it does not do is the thing the user thought it did, which is what the
// finding is for.
//
// The specifier is matched as a string literal because that is what survives
// bundling: `blitsen/*` stays external — nothing in the export can inline the
// runtime behind it — so `from "blitsen/clipboard"`, `import("blitsen/clipboard")`
// and `require("blitsen/clipboard")` all leave the same quoted specifier behind.
function nativeModuleRules(target) {
  return absentNativeModules(target).map(({ module, platform, reason }) => [
    "NATIVE_MODULE_ABSENT",
    "warning",
    new RegExp(`["'\`]blitsen/${module}["'\`]`, "g"),
    `blitsen/${module} does not exist on ${platform}.`,
    `${reason} Every member reads as undefined, so feature-detect the ones you use `
      + `— if (${module}.x) selects a fallback — or build for a target that has the module.`,
  ]);
}

// Raw HID needs access no line of application code can grant itself (#247): a
// udev rule on Linux, an entitlement in the macOS signature, and on Windows a
// set of collections the system keeps whatever the packaging says. None of that
// is a mistake in the source, so this is not an absence and not an error — it
// is the part of shipping a HID application that happens outside the editor,
// said at the moment the target is known. Matched on the same quoted specifier
// `nativeModuleRules` matches, and for the same reason: `blitsen/*` stays
// external, so the literal survives bundling.
//
// Nothing is reported on a platform where the module does not exist at all; the
// absence finding already covers that and says more.
function nativeHidRules(target) {
  const platform = platformOf(target);
  const requirement = HID_ACCESS[platform];
  if (!requirement || absentNativeModules(target).some(entry => entry.module === "hid")) return [];
  return [[
    "NATIVE_HID_ACCESS",
    "warning",
    /["'`]blitsen\/hid["'`]/g,
    `blitsen/hid opens devices that the ${platform} install has to grant access to.`,
    `${requirement} Until it does, open() rejects with a NotAllowedError naming the device.`,
  ]];
}

function position(source, index) {
  const before = source.slice(0, index);
  const lines = before.split("\n");
  return { line: lines.length, column: lines.at(-1).length + 1 };
}

function scanRules(source, file, rules) {
  const diagnostics = [];
  for (const [code, severity, expression, message, guidance] of rules) {
    expression.lastIndex = 0;
    let match;
    while ((match = expression.exec(source))) {
      // A rule that captures is asking a question the regex cannot finish
      // answering; `target` carries what it captured to whoever can.
      const target = match[1];
      diagnostics.push({ file, ...position(source, match.index), severity, code, message, guidance,
        ...(target === undefined ? {} : { target }) });
      if (match[0].length === 0) expression.lastIndex += 1;
    }
  }
  return diagnostics;
}

/// Every file the output ships, as document-relative paths.
///
/// Not the scannable subset: what `fetch` names is usually data or media, and
/// the question here is whether the export carries it, not whether it is code.
async function collectShippedPaths(root) {
  return new Set((await walkFiles(root)).map(file => file.relative));
}

// A fetch URL resolves against the document, which sits at the output root —
// so `./data.json` and `/data.json` name the same file, and both are answered
// from the export when it carries one. Query and fragment are not part of the
// file's name, exactly as a server would drop them before opening it.
//
// A relative one is also tried against the file it was written in, because
// `fetch(new URL("./blip.wav", import.meta.url))` resolves against the module
// rather than against the document, and a chunk in `assets/` naming its own
// neighbour is the common case. Only ever an extra way to *not* report: a
// finding survives only when neither reading of it names a shipped file.
function namesAShippedFile(target, shipped, file) {
  if (target === undefined) return false;
  const path = target.split(/[?#]/)[0];
  if (path === "") return false;
  if (shipped.has(path.replace(/^\.?\//, ""))) return true;
  if (path.startsWith("/")) return false;
  const directory = file.split("/").slice(0, -1);
  const segments = path.split("/");
  for (const segment of segments) {
    if (segment === "." || segment === "") continue;
    if (segment === "..") directory.pop();
    else directory.push(segment);
  }
  return shipped.has(directory.join("/"));
}

async function collectScannableFiles(root) {
  return walkFiles(root, {
    filter: file => SCANNABLE_EXTENSIONS.includes(extname(file.relative).toLowerCase()),
  });
}

/**
 * Grades built output against the v1 profile, and against `target`'s own
 * `native:` surface.
 *
 * `target` defaults to the host because that is what an unqualified `blitsen
 * doctor` is asking about, and it is what `blitsen build` without `--target`
 * goes on to produce. A cross-target build passes the target it is building
 * for, so the modules graded are the ones that will actually be there.
 */
export async function doctorApplication(root, { target = hostTarget() } = {}) {
  const files = await collectScannableFiles(root);
  const shipped = await collectShippedPaths(root);
  const { javascript, css, html } = await compatibility();
  const scripts = [...javascript, ...nativeModuleRules(target), ...nativeHidRules(target)];
  const diagnostics = [];
  for (const file of files) {
    const source = await readFile(file.absolute, "utf8");
    const extension = extname(file.relative).toLowerCase();
    if (HTML_EXTENSIONS.includes(extension)) {
      diagnostics.push(...scanRules(source, file.relative, html));
    } else if (extension === ".css") {
      diagnostics.push(...scanRules(source, file.relative, css));
    } else {
      diagnostics.push(...scanRules(source, file.relative, scripts));
    }
  }
  // Reported only for what the export does not carry: reading a file the
  // application shipped is what `fetch` is for now, not a finding (issue #125).
  const reported = diagnostics.filter(diagnostic =>
    diagnostic.code !== "WEB_FETCH"
    || !namesAShippedFile(diagnostic.target, shipped, diagnostic.file));
  reported.sort((left, right) => left.file.localeCompare(right.file)
    || left.line - right.line || left.column - right.column || left.code.localeCompare(right.code));
  return {
    profile: "v1-strict",
    target,
    files: files.length,
    diagnostics: reported,
    errors: reported.filter(item => item.severity === "error").length,
    warnings: reported.filter(item => item.severity === "warning").length,
  };
}

export function formatDiagnostic(diagnostic) {
  return `${diagnostic.file}:${diagnostic.line}:${diagnostic.column} `
    + `[${diagnostic.severity} ${diagnostic.code}] ${diagnostic.message} ${diagnostic.guidance}`;
}

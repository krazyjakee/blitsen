import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { absentNativeModules, NATIVE_PLATFORMS } from "../native-modules.mjs";

export const COMPATIBILITY_DOC = join(import.meta.dirname, "../../../../docs/COMPATIBILITY.md");

// The published type definitions, checked against the manifest rather than
// maintained beside it (issue #74).
//
// The failure this prevents is the one that costs a user the most: editor
// completion offering `blitsen/window.create`, the code compiling, and the call
// returning `undefined` at run time. So the rule is exact in both directions —
// a declared member the runtime does not install is a promise, and an installed
// member left undeclared is completion the user does not get.
//
// Each `blitsen/<module>` subpath has its own declaration file. The interface
// it names carries only the members this version actually installs.
const TYPE_DEFINITIONS = join(import.meta.dirname, "../native/native.d.ts");
const MODULE_INTERFACES = { app: "NativeApp", window: "NativeWindow",
  dialog: "NativeDialog", clipboard: "NativeClipboard", tray: "NativeTray",
  menu: "NativeMenu", input: "NativeInput", hid: "NativeHid", notify: "NativeNotify", os: "NativeOs" };

/** Reads the members each `Native*` interface declares, by module. */
export function readDeclaredNativeMembers(definitions) {
  const declared = new Map();
  for (const [module, interfaceName] of Object.entries(MODULE_INTERFACES)) {
    const opening = `export interface ${interfaceName} {\n`;
    const start = definitions.indexOf(opening);
    if (start < 0) throw new Error(`native.d.ts no longer declares ${interfaceName}`);
    const end = definitions.indexOf("\n}", start);
    if (end < 0) throw new Error(`${interfaceName} is not a closed interface`);
    const body = definitions.slice(start + opening.length, end);
    // Exactly two spaces: a member of this interface, rather than a field of an
    // inline object type inside one of its signatures.
    declared.set(module, new Set([...body.matchAll(/^ {2}(?:readonly )?([A-Za-z_$][\w$]*)\??[(:<]/gm)]
      .map(([, member]) => member)));
  }
  return declared;
}

/**
 * Refuses type definitions and a manifest that disagree.
 *
 * Returns the number of members checked, so a caller can tell a pass from a
 * check that matched nothing because the reader stopped working.
 */
export function checkTypeDefinitions(manifest, definitions) {
  const declared = readDeclaredNativeMembers(definitions);
  const problems = [];
  let checked = 0;
  for (const [module, members] of declared) {
    const implemented = new Set(manifest.native
      .filter(entry => entry.module === module && entry.status === "implemented")
      .map(entry => entry.member));
    for (const member of members) {
      checked += 1;
      if (!implemented.has(member))
        problems.push(`blitsen/${module} declares ${member}, which the runtime does not install`);
    }
    for (const member of implemented)
      if (!members.has(member))
        problems.push(`blitsen/${module} installs ${member}, which native.d.ts does not declare`);
  }
  // A module the definitions give no interface must have nothing installed:
  // otherwise its subpath types as empty while the runtime answers.
  for (const entry of manifest.native)
    if (entry.status === "implemented" && !declared.has(entry.module))
      problems.push(`blitsen/${entry.module} installs ${entry.member} and has no declared interface`);
  if (problems.length > 0)
    throw new Error(`the published types and the runtime disagree:\n  ${problems.join("\n  ")}`);
  return checked;
}

export async function checkPublishedTypes(manifest) {
  return checkTypeDefinitions(manifest, await readFile(TYPE_DEFINITIONS, "utf8"));
}

const names = entries => entries.map(entry => `\`${entry.api}\``).join(", ") || "—";

// Renders the capability tiers documented in COMPATIBILITY.md.
export function renderCapabilityTiers(manifest) {
  const codes = [...new Set(manifest.apis.map(entry => entry.code))];
  const surface = codes.map(code => {
    const entries = manifest.apis.filter(entry => entry.code === code);
    return `| ${code} | ${names(entries.filter(entry => entry.status === "implemented"))} `
      + `| ${names(entries.filter(entry => entry.status === "absent"))} |`;
  });
  // Implemented, and on the platforms named decided per process rather than per
  // build — which is why these keep a table of their own: the row above says an
  // API is installed, and for one platform that is true of one run of a build
  // and not of another.
  const conditional = manifest.apis.filter(entry => entry.condition).map(entry =>
    `| \`${entry.api}\` | ${entry.condition.platforms.join(", ")} | ${entry.condition.reason} |`);
  const diagnosed = [
    ...manifest.usage,
    ...codes
      .filter(code => manifest.apis.some(entry => entry.code === code && entry.status === "absent"))
      .map(code => ({ code, ...manifest.diagnostics[code] })),
    ...manifest.renderer,
    ...manifest.assets,
  ]
    // A code declared for more than one file kind is still one diagnostic.
    .filter((rule, index, rules) => rules.findIndex(other => other.code === rule.code) === index)
    .map(rule => `| \`${rule.code}\` | ${rule.severity} | ${rule.message} |`);
  return ["| Group | Implemented | Absent |", "| --- | --- | --- |", ...surface, "",
    "| Conditional API | Platform | Installed when |", "| --- | --- | --- |", ...conditional, "",
    "| Diagnostic | Severity | Reported as |", "| --- | --- | --- |", ...diagnosed].join("\n");
}

// Renders the `blitsen/*` module surface documented in COMPATIBILITY.md.
export function renderNativeModules(manifest) {
  const modules = [...new Set(manifest.native.map(entry => entry.module))];
  const members = (module, status) => manifest.native
    .filter(entry => entry.module === module && entry.status === status)
    .map(entry => `\`${entry.member}\``).join(", ") || "—";
  const surface = modules.map(module => `| \`blitsen/${module}\` `
    + `| ${members(module, "implemented")} | ${members(module, "absent")} |`);
  const absent = manifest.native.filter(entry => entry.status === "absent")
    .map(entry => `| \`${entry.api}\` | ${entry.reason} |`);
  const conditional = manifest.native.filter(entry => entry.condition)
    .map(entry => `| \`${entry.api}\` | ${entry.condition.platforms.join(", ")} `
      + `| ${entry.condition.reason} |`);
  const unavailableModules = NATIVE_PLATFORMS.flatMap(platform =>
    absentNativeModules(`${platform}-x64`).map(entry =>
      `| \`blitsen/${entry.module}\` | ${platform} | ${entry.reason} |`));
  return ["| Module | Implemented | Absent |", "| --- | --- | --- |", ...surface, "",
    "| Absent member | Why |", "| --- | --- |", ...absent, "",
    "| Native module | Platform where absent | Why |", "| --- | --- | --- |",
    ...unavailableModules, "",
    "| Conditional native member | Platform where absent | Why |", "| --- | --- | --- |",
    ...conditional].join("\n");
}

function replaceGenerated(document, section, body) {
  const open = `<!-- generated: ${section} -->`;
  const close = "<!-- /generated -->";
  const start = document.indexOf(open);
  const end = document.indexOf(close, start);
  if (start < 0 || end < 0) throw new Error(`COMPATIBILITY.md has no ${section} generated section`);
  return `${document.slice(0, start + open.length)}\n\n${body}\n\n${document.slice(end)}`;
}

export async function renderCompatibilityDoc(manifest) {
  const document = replaceGenerated(await readFile(COMPATIBILITY_DOC, "utf8"), "api-manifest",
    renderCapabilityTiers(manifest));
  return replaceGenerated(document, "native-modules", renderNativeModules(manifest));
}

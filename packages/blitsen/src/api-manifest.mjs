import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

import {
  ASSET_RULES, CATALOGUE, CONDITIONAL, DECLARED, DIAGNOSTICS, ENGINE_ABSENT, NATIVE,
  NATIVE_ABSENT, NATIVE_CONDITIONAL, RENDERER_RULES, USAGE_RULES,
} from "./api-manifest/catalogue.mjs";
import {
  SOURCE_NAME, extractRuntimeSurface, readBootstrapScript,
} from "./api-manifest/source-scanner.mjs";
import {
  COMPATIBILITY_DOC, checkPublishedTypes, checkTypeDefinitions, readDeclaredNativeMembers,
  renderCapabilityTiers, renderCompatibilityDoc, renderNativeModules,
} from "./api-manifest/rendering.mjs";
import { NATIVE_PLATFORMS } from "./native-modules.mjs";

export { extractRuntimeSurface, readBootstrapScript };
export {
  checkPublishedTypes, checkTypeDefinitions, readDeclaredNativeMembers,
  renderCapabilityTiers, renderCompatibilityDoc, renderNativeModules,
};

// Paths rather than URL objects, here and everywhere else this package reads a
// file of its own: the DOM bridge installs Blitsen's `URL` over the host's in
// the realm the CLI shares with it, and `node:fs` accepts only the host's.
const MANIFEST_FILE = join(import.meta.dirname, "./api-manifest.json");

function memberOwner(surface, owner) {
  const className = surface.instances.get(owner) ?? owner;
  if (!surface.classes.has(className)) return null;
  const members = new Set();
  for (let name = className; surface.classes.has(name); name = surface.classes.get(name).base)
    for (const member of surface.classes.get(name).members) members.add(member);
  return { target: surface.instances.has(owner) ? owner : `${owner}.prototype`, members };
}

function apiEntry(surface, entry, code) {
  const [api, override] = Array.isArray(entry) ? entry : [entry, undefined];
  const [owner, member] = api.includes(".") ? api.split(".") : [null, null];
  if (!owner) {
    return { api, kind: "global", status: surface.globals.includes(api) ? "implemented" : "absent",
      code, pattern: override === undefined ? `(?<![.\\w$])${api}\\b` : override };
  }
  const resolved = memberOwner(surface, owner);
  if (!resolved) throw new Error(`the bootstrap has no ${owner} to look ${api} up on`);
  // Only a member read off a named global can be matched in a bundle; one read
  // off an instance the application named itself cannot.
  const pattern = surface.instances.has(owner) ? `\\b${owner}\\.${member}\\b` : null;
  return { api, kind: "member", owner: resolved.target, member,
    status: resolved.members.has(member) ? "implemented" : "absent", code,
    pattern: override === undefined ? pattern : override };
}

// Reads the `native:` surface out of the bootstrap, refusing anything the two
// disagree about — an installed member this file does not declare, or an absent
// one it cannot say why about.
function nativeEntries(surface) {
  const entries = Object.entries(NATIVE).flatMap(([module, members]) => {
    const installed = surface.native.get(module);
    const undeclared = [...installed].filter(member => !members.includes(member));
    if (undeclared.length > 0)
      throw new Error(`${SOURCE_NAME} installs native:${module}.`
        + `${undeclared.join(`, native:${module}.`)}, which this manifest does not declare; `
        + "add each one to NATIVE");
    return members.map(member => {
      const api = `${module}.${member}`;
      const status = installed.has(member) ? "implemented" : "absent";
      const reason = NATIVE_ABSENT[api];
      if (status === "absent" && !reason)
        throw new Error(`native:${api} is not installed and NATIVE_ABSENT does not say why`);
      if (status === "implemented" && reason)
        throw new Error(`native:${api} is installed, so NATIVE_ABSENT must not explain it away`);
      const condition = NATIVE_CONDITIONAL[api];
      if (condition) {
        const unknown = condition.platforms
          .filter(platform => !NATIVE_PLATFORMS.includes(platform));
        if (unknown.length > 0)
          throw new Error(`native:${api} is conditional on ${unknown.join(", ")}, which is not a platform`);
        if (status !== "implemented")
          throw new Error(`native:${api} is conditional and not implemented on any platform`);
      }
      return { api, module, member, status, ...(reason ? { reason } : {}),
        ...(condition ? { condition } : {}) };
    });
  });
  return entries;
}

// A rule matched against a source file of one kind, rather than against an API.
const sourceScanRule = ([kind, code, severity, pattern, message, guidance]) =>
  ({ kind, code, severity, pattern, message, guidance });

// Builds the manifest from the bootstrap script, refusing anything the two disagree about.
export function buildManifest(script) {
  const surface = extractRuntimeSurface(script);
  const apis = Object.entries(CATALOGUE)
    .flatMap(([code, names]) => names.map(api => apiEntry(surface, api, code)));
  // Declared, not derived: see ENGINE_ABSENT. Appended after the invariants
  // below have run against the bridge's own surface, so an engine absence
  // cannot be mistaken for a bridge one.
  const engineApis = Object.entries(ENGINE_ABSENT).flatMap(([code, names]) => names.map(api =>
    ({ api, kind: "global", status: "absent", origin: "engine", code,
      pattern: `(?<![.\\w$])${api}\\b` })));
  // Declared members of an installed global. The pattern is the dotted name a
  // bundle actually writes, escaped, so `doctor` finds `Intl.Segmenter` without
  // matching every other `Intl.` reference.
  const declaredApis = Object.entries(DECLARED).flatMap(([code, members]) =>
    members.map(([api, owner, member, implemented]) => ({
      api, kind: "member", owner, member,
      status: implemented ? "implemented" : "absent", code,
      pattern: implemented ? null : `\\b${api.replace(/\./g, "\\.")}\\b`,
    })));

  const described = new Set(apis.filter(entry => entry.kind === "global").map(entry => entry.api));
  const undescribed = surface.globals.filter(name => !described.has(name));
  if (undescribed.length > 0)
    throw new Error(`${SOURCE_NAME} installs ${undescribed.join(", ")}, which this manifest `
      + "does not describe; add each one to CATALOGUE");
  const absent = apis.filter(entry => entry.kind === "global" && entry.status === "absent");
  const undeleted = absent.map(entry => entry.api).filter(api => !surface.deleted.includes(api));
  if (undeleted.length > 0)
    throw new Error(`${undeleted.join(", ")} are absent from the runtime but not deleted by the `
      + "bootstrap, so the Phase 1 host can supply its own");
  const overdeleted = surface.deleted.filter(name => !absent.some(entry => entry.api === name));
  if (overdeleted.length > 0)
    throw new Error(`the bootstrap deletes ${overdeleted.join(", ")}, which the manifest does not `
      + "describe as absent");
  for (const entry of apis)
    if (entry.status === "absent" && !DIAGNOSTICS[entry.code])
      throw new Error(`${entry.api} is absent and ${entry.code} has no diagnostic to report it`);
  // The two halves of a condition, checked against each other the way an absence
  // and its deletion are above: the manifest may only call an API conditional if
  // the bootstrap can withdraw it, and may not leave a withdrawal unexplained.
  for (const [api, { platforms, reason }] of Object.entries(CONDITIONAL)) {
    const entry = apis.find(candidate => candidate.api === api);
    if (entry?.status !== "implemented")
      throw new Error(`${api} is declared conditional and is not an implemented API`);
    if (!surface.conditional.includes(api))
      throw new Error(`${api} is declared conditional and the bootstrap installs it whatever the `
        + "host answered, so nothing decides it at run time");
    const unknown = platforms.filter(platform => !NATIVE_PLATFORMS.includes(platform));
    if (unknown.length > 0)
      throw new Error(`${api} is conditional on ${unknown.join(", ")}, which is not a platform`);
    entry.condition = { platforms, reason };
  }
  const unexplained = surface.conditional.filter(name => !CONDITIONAL[name]);
  if (unexplained.length > 0)
    throw new Error(`the bootstrap withdraws ${unexplained.join(", ")} when the host cannot carry `
      + "it, and CONDITIONAL does not say on which platforms or why");

  return {
    generatedBy: `packages/blitsen/src/api-manifest.mjs from ${SOURCE_NAME}`,
    profile: "v1-strict",
    apis: [...apis, ...engineApis, ...declaredApis],
    native: nativeEntries(surface),
    diagnostics: Object.fromEntries(Object.entries(DIAGNOSTICS)
      .map(([code, [severity, message, guidance, extra]]) =>
        [code, { severity, message, guidance, extra: extra?.source ?? null }])),
    usage: USAGE_RULES.map(([code, severity, pattern, message, guidance]) =>
      ({ code, severity, pattern, message, guidance })),
    renderer: RENDERER_RULES.map(sourceScanRule),
    assets: ASSET_RULES.map(sourceScanRule),
  };
}

// Loads the generated manifest. The runtime source is not published; this is.
export async function loadApiManifest() {
  return JSON.parse(await readFile(MANIFEST_FILE, "utf8"));
}

export async function generateApiManifest() {
  return buildManifest(await readBootstrapScript());
}

if (import.meta.main) {
  const manifest = await generateApiManifest();
  await writeFile(MANIFEST_FILE, `${JSON.stringify(manifest, null, 2)}\n`);
  await writeFile(COMPATIBILITY_DOC, await renderCompatibilityDoc(manifest));
  const absent = manifest.apis.filter(entry => entry.status === "absent").length;
  const nativeAbsent = manifest.native.filter(entry => entry.status === "absent").length;
  const typed = await checkPublishedTypes(manifest);
  console.log(`api-manifest: ${manifest.apis.length - absent} implemented, ${absent} absent APIs `
    + `and ${manifest.native.length - nativeAbsent} implemented, ${nativeAbsent} absent native `
    + `members read from ${SOURCE_NAME}; ${typed} declared members agree with them`);
}

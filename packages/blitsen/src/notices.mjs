// Third-party notices for an exported application (issue #121).
//
// LICENSING.md's acceptance gate: an export may not claim redistribution
// compliance until the notices it owes travel with the artifact, generated from
// the build rather than maintained by hand, and until a test extracts them from
// a real artifact and finds them complete.
//
// This is the generator. It reads the dependency graph `cargo` resolved for one
// platform, collects what each package's licence requires to travel with it, and
// renders one document. Cargo is needed to *produce* the notices, never to
// consume them: they are generated where the runtime is built — this checkout,
// or the release job that builds a platform package — and shipped inside it, so
// a user's machine needs no toolchain (P9).
//
// Two things the licences in this tree actually demand:
//
//   - **MIT, BSD, ISC, Zlib and friends**: the copyright notice and the
//     permission text travel with the binary. Every package's copyright lines
//     are listed; each distinct licence text appears once, because reproducing
//     the same MIT text 115 times is 115 copies of the same requirement.
//   - **MPL-2.0** (Stylo, and 44 other packages): the covered source has to be
//     available. The offer names the exact revision each one was built from,
//     which is what makes it durable rather than decorative.
import { readdir, readFile, stat, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";

/** What a licence file is called, in the order a package tends to name them. */
const LICENCE_FILES = /^(?:LICEN[CS]E|COPYING|NOTICE|UNLICENSE|COPYRIGHT)/i;
/** A licence text longer than this is a vendored corpus, not a licence. */
const MAX_LICENCE_BYTES = 64 * 1024;

/** Runs `cargo metadata` for `target`, or throws with what to install. */
async function cargoMetadata({ target, manifestPath, run }) {
  const result = await run("cargo", [
    "metadata", "--format-version", "1", "--locked",
    ...(manifestPath ? ["--manifest-path", manifestPath] : []),
    ...(target ? ["--filter-platform", target] : []),
  ]);
  if (result.code !== 0) {
    throw new Error("could not read the dependency graph with `cargo metadata`, which is how "
      + `third-party notices are generated: ${result.stderr.trim()}`);
  }
  return JSON.parse(result.stdout);
}

/**
 * The packages an artifact built from `root` links, transitively.
 *
 * Dev-dependencies are excluded — nothing a test needs is in the binary — and
 * everything else is kept, including build dependencies, because a code
 * generator's licence still applies to the code it generated.
 */
function linkedPackages(metadata, root) {
  const packages = new Map(metadata.packages.map(entry => [entry.id, entry]));
  const nodes = new Map(metadata.resolve.nodes.map(node => [node.id, node]));
  const start = [...packages.values()].find(entry => entry.name === root);
  if (start === undefined) throw new Error(`${root} is not in this workspace`);
  const seen = new Set();
  const pending = [start.id];
  while (pending.length > 0) {
    const id = pending.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const dependency of nodes.get(id)?.deps ?? []) {
      if (dependency.dep_kinds.length > 0
        && dependency.dep_kinds.every(kind => kind.kind === "dev")) continue;
      pending.push(dependency.pkg);
    }
  }
  return [...seen]
    .map(id => packages.get(id))
    .filter(entry => entry !== undefined && entry.id !== start.id)
    .sort((left, right) => left.name.localeCompare(right.name)
      || left.version.localeCompare(right.version));
}

/** Reads the licence texts a package ships beside its manifest. */
async function licenceTexts(entry) {
  const directory = dirname(entry.manifest_path);
  let names;
  try {
    names = (await readdir(directory)).filter(name => LICENCE_FILES.test(name)).sort();
  } catch {
    return [];
  }
  const texts = [];
  for (const name of names) {
    const path = join(directory, name);
    try {
      if ((await stat(path)).size > MAX_LICENCE_BYTES) continue;
      texts.push({ name, text: (await readFile(path, "utf8")).replace(/\r\n/g, "\n").trim() });
    } catch {
      // A directory named LICENSE, or a file this process cannot read: the
      // package's declared SPDX terms still describe it, and the audit below
      // reports what has no text rather than failing the build here.
    }
  }
  return texts;
}

/** The copyright lines a notice has to carry, taken from the licence text. */
function copyrightLines(texts) {
  const lines = new Set();
  for (const { text } of texts) {
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (/^(?:copyright|\(c\)|©)/i.test(trimmed) && trimmed.length < 200) lines.add(trimmed);
    }
  }
  return [...lines];
}

const digest = text => createHash("sha256").update(text).digest("hex").slice(0, 16);

/**
 * Collects everything the notices need, without rendering them.
 *
 * Separated so the audit can ask questions of the data — is anything here
 * without a licence, is every MPL package accounted for — rather than of a
 * string.
 */
export async function collectNotices({
  target = null, manifestPath = null, root = "blitsen-runtime", run,
} = {}) {
  const metadata = await cargoMetadata({ target, manifestPath, run });
  const linked = linkedPackages(metadata, root);
  const packages = [];
  const licences = new Map();
  for (const entry of linked) {
    const texts = await licenceTexts(entry);
    const keys = [];
    for (const { name, text } of texts) {
      const key = digest(text);
      if (!licences.has(key)) licences.set(key, { key, name, text, packages: [] });
      licences.get(key).packages.push(`${entry.name} ${entry.version}`);
      keys.push(key);
    }
    packages.push({
      name: entry.name,
      version: entry.version,
      license: entry.license ?? null,
      licenseFile: entry.license_file ?? null,
      repository: entry.repository ?? null,
      // A git dependency carries the revision it was built from in its source
      // string, which is what makes an MPL source offer point at something.
      source: entry.source ?? null,
      copyright: copyrightLines(texts),
      texts: keys,
    });
  }
  return { target, root, packages, licences: [...licences.values()] };
}

/** Packages whose terms this document cannot honour, and why. */
function auditNotices(collected) {
  const problems = [];
  for (const entry of collected.packages) {
    if (entry.license === null && entry.licenseFile === null && entry.texts.length === 0) {
      problems.push(`${entry.name} ${entry.version} declares no licence and ships no licence file`);
    }
    if (/MPL-2\.0/i.test(entry.license ?? "") && entry.repository === null && entry.source === null) {
      problems.push(`${entry.name} ${entry.version} is MPL-2.0 with no source anyone can reach`);
    }
  }
  return problems;
}

/** The distributable document, as plain text. */
function renderNotices(collected, { version = null } = {}) {
  const lines = [];
  lines.push("THIRD-PARTY NOTICES");
  lines.push("");
  lines.push("This application was built with Blitsen"
    + `${version ? ` ${version}` : ""}, which links the open-source packages listed`);
  lines.push(`below${collected.target ? ` for ${collected.target}` : ""}. `
    + "Their terms apply to this executable; the application's own HTML, CSS and");
  lines.push("JavaScript is an interpreted payload and is not covered by them.");
  lines.push("");
  lines.push("Generated from the dependency graph the binary was built from, by");
  lines.push("packages/blitsen/src/notices.mjs. It is not maintained by hand.");
  lines.push("");
  const mpl = collected.packages.filter(entry => /MPL-2\.0/i.test(entry.license ?? ""));
  if (mpl.length > 0) {
    lines.push("SOURCE OFFER (MPL-2.0)");
    lines.push("");
    lines.push(`${mpl.length} of the packages below are covered by the Mozilla Public License 2.0,`);
    lines.push("which requires the covered source to be available. Each is listed with the exact");
    lines.push("revision this binary was built from; the source is available from the repository");
    lines.push("named there, and on request from the distributor of this application.");
    lines.push("");
    for (const entry of mpl) {
      lines.push(`  ${entry.name} ${entry.version} — ${entry.source ?? entry.repository}`);
    }
    lines.push("");
  }
  lines.push(`PACKAGES (${collected.packages.length})`);
  lines.push("");
  for (const entry of collected.packages) {
    lines.push(`${entry.name} ${entry.version} — ${entry.license ?? entry.licenseFile ?? "see below"}`);
    if (entry.repository) lines.push(`  ${entry.repository}`);
    for (const line of entry.copyright) lines.push(`  ${line}`);
  }
  lines.push("");
  lines.push(`LICENCE TEXTS (${collected.licences.length})`);
  lines.push("");
  lines.push("Each distinct text appears once, followed by the packages it came with.");
  for (const licence of collected.licences) {
    lines.push("");
    lines.push("-".repeat(78));
    lines.push(`${licence.name} — ${licence.packages.length} package(s): `
      + `${licence.packages.join(", ")}`);
    lines.push("-".repeat(78));
    lines.push(licence.text);
  }
  lines.push("");
  return `${lines.join("\n")}\n`;
}

/**
 * Writes `NOTICES.txt` and `NOTICES.json` beside a built runtime.
 *
 * The pair is what a platform package ships: the text is what an export
 * carries, and the JSON is the audited manifest for that target — one per
 * platform package, which is what the gate asks for.
 */
export async function writeNotices(directory, collected, { version = null } = {}) {
  const problems = auditNotices(collected);
  const text = join(directory, "NOTICES.txt");
  const data = join(directory, "NOTICES.json");
  await writeFile(text, renderNotices(collected, { version }));
  await writeFile(data, `${JSON.stringify({
    target: collected.target,
    root: collected.root,
    generatedFor: version,
    packages: collected.packages.map(({ texts, ...rest }) => rest),
    problems,
  }, null, 2)}\n`);
  return { text, data, problems, packages: collected.packages.length };
}

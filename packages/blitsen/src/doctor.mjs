import { readFile, readdir } from "node:fs/promises";
import { extname, join, relative, sep } from "node:path";
import { loadApiManifest } from "./api-manifest.mjs";

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
async function collectShippedPaths(root, directory = root, paths = new Set()) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.isSymbolicLink()) continue;
    const absolute = join(directory, entry.name);
    if (entry.isDirectory()) await collectShippedPaths(root, absolute, paths);
    else paths.add(relative(root, absolute).split(sep).join("/"));
  }
  return paths;
}

// A fetch URL resolves against the document, which sits at the output root —
// so `./data.json` and `/data.json` name the same file, and both are answered
// from the export when it carries one. Query and fragment are not part of the
// file's name, exactly as a server would drop them before opening it.
function namesAShippedFile(target, shipped) {
  if (target === undefined) return false;
  const path = target.split(/[?#]/)[0].replace(/^\.?\//, "");
  return path !== "" && shipped.has(path);
}

async function collectScannableFiles(root, directory = root) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const absolute = join(directory, entry.name);
    if (entry.isSymbolicLink()) continue;
    if (entry.isDirectory()) files.push(...await collectScannableFiles(root, absolute));
    else if ([".html", ".htm", ".css", ".js", ".mjs", ".cjs"].includes(extname(entry.name).toLowerCase())) {
      files.push({ absolute, relative: relative(root, absolute).split(sep).join("/") });
    }
  }
  return files.sort((left, right) => left.relative.localeCompare(right.relative));
}

export async function doctorApplication(root) {
  const files = await collectScannableFiles(root);
  const shipped = await collectShippedPaths(root);
  const { javascript, css, html } = await compatibility();
  const diagnostics = [];
  for (const file of files) {
    const source = await readFile(file.absolute, "utf8");
    const extension = extname(file.relative).toLowerCase();
    if ([".html", ".htm"].includes(extension)) {
      diagnostics.push(...scanRules(source, file.relative, html));
    } else if (extension === ".css") {
      diagnostics.push(...scanRules(source, file.relative, css));
    } else {
      diagnostics.push(...scanRules(source, file.relative, javascript));
    }
  }
  // Reported only for what the export does not carry: reading a file the
  // application shipped is what `fetch` is for now, not a finding (issue #125).
  const reported = diagnostics.filter(diagnostic =>
    diagnostic.code !== "WEB_FETCH" || !namesAShippedFile(diagnostic.target, shipped));
  reported.sort((left, right) => left.file.localeCompare(right.file)
    || left.line - right.line || left.column - right.column || left.code.localeCompare(right.code));
  return {
    profile: "v0-strict",
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

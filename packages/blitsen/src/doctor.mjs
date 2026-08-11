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
  const renderer = kind => manifest.renderer.filter(rule => rule.kind === kind).map(rule =>
    [rule.code, rule.severity, new RegExp(rule.pattern, "gi"), rule.message, rule.guidance]);
  return { javascript, css: renderer("css"), html: renderer("html") };
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
      diagnostics.push({ file, ...position(source, match.index), severity, code, message, guidance });
      if (match[0].length === 0) expression.lastIndex += 1;
    }
  }
  return diagnostics;
}

function scanExternalAssets(source, file, kind) {
  const expression = kind === ".css"
    ? /url\(\s*["']?(https?:\/\/|\/\/)/gi
    : /<(?:script|img|source|audio|video|track|embed|input)\b[^>]*\bsrc\s*=\s*["'](https?:\/\/|\/\/)|<link\b[^>]*\bhref\s*=\s*["'](https?:\/\/|\/\/)|<video\b[^>]*\bposter\s*=\s*["'](https?:\/\/|\/\/)|<object\b[^>]*\bdata\s*=\s*["'](https?:\/\/|\/\/)/gi;
  const diagnostics = [];
  let match;
  while ((match = expression.exec(source))) diagnostics.push({
    file, ...position(source, match.index), severity: "error", code: "ASSET_REMOTE",
    message: "Remote assets are not part of a self-contained static export.",
    guidance: "Bundle the asset into the output directory and reference its local path.",
  });
  return diagnostics;
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
  const { javascript, css, html } = await compatibility();
  const diagnostics = [];
  for (const file of files) {
    const source = await readFile(file.absolute, "utf8");
    const extension = extname(file.relative).toLowerCase();
    if ([".html", ".htm"].includes(extension)) {
      diagnostics.push(...scanRules(source, file.relative, html));
      diagnostics.push(...scanExternalAssets(source, file.relative, extension));
    } else if (extension === ".css") {
      diagnostics.push(...scanRules(source, file.relative, css));
      diagnostics.push(...scanExternalAssets(source, file.relative, extension));
    } else {
      diagnostics.push(...scanRules(source, file.relative, javascript));
    }
  }
  diagnostics.sort((left, right) => left.file.localeCompare(right.file)
    || left.line - right.line || left.column - right.column || left.code.localeCompare(right.code));
  return {
    profile: "v0-strict",
    files: files.length,
    diagnostics,
    errors: diagnostics.filter(item => item.severity === "error").length,
    warnings: diagnostics.filter(item => item.severity === "warning").length,
  };
}

export function formatDiagnostic(diagnostic) {
  return `${diagnostic.file}:${diagnostic.line}:${diagnostic.column} `
    + `[${diagnostic.severity} ${diagnostic.code}] ${diagnostic.message} ${diagnostic.guidance}`;
}

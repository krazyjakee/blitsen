import { readFile } from "node:fs/promises";
import { extname, join, posix, relative } from "node:path";
import {
  CSS_REFERENCE_PATTERNS, CSS_ROOT_REFERENCE_PATTERNS,
  HTML_REFERENCE_PATTERNS, HTML_ROOT_REFERENCE_PATTERNS,
} from "./asset-references.mjs";
import { HTML_EXTENSIONS, SCRIPT_EXTENSIONS, walkFiles } from "./files.mjs";

// Statically analysable module-graph edges only. Computed specifiers are not
// followed; --include is the escape hatch for those.
const SCRIPT_REFERENCES = [
  /\bfrom\s*["']([^"']*)["']/g,
  /\bimport\s*\(?\s*["']([^"']*)["']/g,
  /\bnew\s+URL\s*\(\s*["']([^"']*)["']\s*,\s*import\.meta\.url\s*\)/g,
];
// A bundler resolves `import hero from "./hero.png"` into a bare string literal,
// so no import edge survives for the walk to follow. Every quoted string is
// therefore offered as a candidate, and only kept when it resolves to a file the
// bundler actually emitted — which is why this cannot invent a reference to
// something that is not there. The asymmetry justifies the guess: a false
// positive costs bytes in the export, a false negative costs a missing asset.
const SCRIPT_LITERALS = [
  /"([^"\n\\]*)"/g,
  /'([^'\n\\]*)'/g,
  /`([^`\n\\$]*)`/g,
];

function referencePatterns(file) {
  const extension = extname(file).toLowerCase();
  if (HTML_EXTENSIONS.includes(extension)) return HTML_REFERENCE_PATTERNS;
  if (extension === ".css") return CSS_REFERENCE_PATTERNS;
  if (SCRIPT_EXTENSIONS.includes(extension)) return [...SCRIPT_REFERENCES, ...SCRIPT_LITERALS];
  return null;
}

function localReference(url) {
  const trimmed = url.trim();
  if (!trimmed || trimmed.startsWith("#") || trimmed.startsWith("//")) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(trimmed)) return null;
  const split = trimmed.search(/[?#]/);
  const pathname = split < 0 ? trimmed : trimmed.slice(0, split);
  return pathname ? { pathname, suffix: split < 0 ? "" : trimmed.slice(split) } : null;
}

// A configured bundler base ("/app/") prefixes server-root URLs with directories
// that do not exist in the output, so drop leading segments until one resolves.
function resolveReference(pathname, sourceFile, exists) {
  if (pathname.startsWith("/")) {
    const segments = pathname.split("/").filter(Boolean);
    for (let index = 0; index < segments.length; index += 1) {
      const candidate = segments.slice(index).join("/");
      if (exists(candidate)) return candidate;
    }
    return null;
  }
  const target = posix.normalize(posix.join(posix.dirname(sourceFile), pathname));
  return target.startsWith("..") || !exists(target) ? null : target;
}

function relativeAssetUrl(target, sourceFile) {
  let rewritten = posix.relative(posix.dirname(sourceFile), target);
  if (!rewritten) rewritten = ".";
  if (!rewritten.startsWith(".")) rewritten = `./${rewritten}`;
  return rewritten;
}

export function rewriteRootRelativeReferences(source, sourceFile, resolveTarget = path => path.slice(1)) {
  const rewrite = (match, prefix, url, suffix = "") => {
    const reference = localReference(url);
    const target = reference && resolveTarget(reference.pathname);
    if (!target) return match;
    return `${prefix}${relativeAssetUrl(target, sourceFile)}${reference.suffix}${suffix}`;
  };
  if (HTML_EXTENSIONS.includes(extname(sourceFile).toLowerCase())) {
    return HTML_ROOT_REFERENCE_PATTERNS
      .reduce((rewritten, pattern) => rewritten.replace(pattern, rewrite), source);
  }
  if (extname(sourceFile).toLowerCase() === ".css") {
    return CSS_ROOT_REFERENCE_PATTERNS
      .reduce((rewritten, pattern) => rewritten.replace(pattern, rewrite), source);
  }
  return source;
}

export function globMatcher(pattern) {
  const expression = pattern.split("**")
    .map(part => part
      .replace(/[.+^${}()|[\]\\]/g, "\\$&")
      .replace(/\*/g, "[^/]*")
      .replace(/\?/g, "[^/]"))
    .join(".*");
  return new RegExp(`^${expression}$`);
}

// Reachability from the HTML entrypoint. Unreferenced output is pure export size,
// so it is reported and dropped unless an --include glob asks for it.
export async function planIngest(root, { entrypoint = "index.html", include = [] } = {}) {
  const files = await walkFiles(root, {
    filter: (_file, entry) => entry.isFile(),
    onSymlink: file => {
      throw new Error(`application output contains a symbolic link: ${relative(root, file.absolute)}`);
    },
  });
  const byPath = new Map(files.map(file => [file.relative, file]));
  if (!byPath.has(entrypoint)) {
    throw new Error(`missing application entrypoint: ${join(root, entrypoint)}`);
  }
  const exists = path => byPath.has(path);
  const resolutions = new Map();
  const reachable = new Set([entrypoint]);
  const unresolved = [];
  const queue = [entrypoint];
  for (let cursor = 0; cursor < queue.length; cursor += 1) {
    const current = queue[cursor];
    const patterns = referencePatterns(current);
    if (!patterns) continue;
    // Script scanning is heuristic, so a string that merely looks like a
    // specifier must not fail an otherwise valid build.
    const authoritative = !SCRIPT_EXTENSIONS.includes(extname(current).toLowerCase());
    const source = await readFile(byPath.get(current).absolute, "utf8");
    const rewrites = new Map();
    resolutions.set(current, rewrites);
    for (const pattern of patterns) {
      pattern.lastIndex = 0;
      let match;
      while ((match = pattern.exec(source))) {
        const reference = localReference(match[1]);
        if (!reference) continue;
        // A chunk-manifest path is written relative to the output root rather
        // than to the chunk holding it, so a literal gets both readings.
        const target = resolveReference(reference.pathname, current, exists)
          ?? (authoritative || reference.pathname.startsWith("/") ? null
            : exists(posix.normalize(reference.pathname)) ? posix.normalize(reference.pathname) : null);
        if (target === null) {
          if (authoritative) unresolved.push({ file: current, url: match[1] });
          continue;
        }
        rewrites.set(reference.pathname, target);
        if (!reachable.has(target)) {
          reachable.add(target);
          queue.push(target);
        }
      }
    }
  }
  if (unresolved.length > 0) {
    const detail = unresolved.map(item => `${item.file} references ${item.url}`).join("\n  ");
    throw new Error(`unresolved local references in the application output:\n  ${detail}`);
  }
  const matchers = include.map(globMatcher);
  const kept = file => reachable.has(file.relative)
    || matchers.some(matcher => matcher.test(file.relative));
  return {
    files: files.filter(kept),
    resolutions,
    unreferenced: files.filter(file => !kept(file)).map(file => file.relative),
  };
}

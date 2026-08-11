import { readFile, readdir } from "node:fs/promises";
import { extname, join, relative, sep } from "node:path";

const JS_RULES = [
  ["WEB_CANVAS", "error", /\b(?:HTMLCanvasElement|CanvasRenderingContext2D|OffscreenCanvas)\b|\.getContext\s*\(/g,
    "Canvas is not in the v0 compatibility profile.", "Use DOM/CSS rendering or a native viewport until canvas support lands."],
  ["WEB_GPU", "error", /\b(?:WebGLRenderingContext|WebGL2RenderingContext|GPUCanvasContext|navigator\.gpu)\b/g,
    "WebGL and WebGPU are not implemented.", "Remove the GPU-web API path or replace it with a native addon/viewport."],
  ["WEB_STORAGE", "error", /\b(?:localStorage|sessionStorage|indexedDB)\b/g,
    "Browser storage is not implemented.", "Use a Node filesystem/database package or feature-detect storage."],
  ["WEB_WORKER", "error", /\b(?:Worker|SharedWorker|ServiceWorkerContainer)\b/g,
    "Web workers are not implemented.", "Run the work in the main context or use a native/Node worker path."],
  ["WEB_NAVIGATION", "error", /\b(?:document\.write|window\.open|location\.(?:assign|replace|reload))\b/g,
    "Document navigation is deliberately absent; there is no page to leave.", "Route with history.pushState and conditional DOM rendering."],
  // `history` and `location` are in profile, but they address a synthetic
  // document with no server behind it, so a literal relative fetch cannot work.
  ["WEB_FETCH", "error", /\bfetch\s*\(\s*["'`](?!https?:\/\/)/g,
    "fetch resolves this URL against an address with no server behind it.", "Bundle the data into the export, or request an absolute http(s) URL."],
  ["WEB_STREAM", "warning", /\b(?:ReadableStream|WritableStream|TransformStream)\b|\.body\s*\.\s*getReader\b/g,
    "Streaming bodies are not implemented; a response is buffered whole.", "Read the response with text(), json(), or arrayBuffer()."],
  ["WEB_SOCKET", "warning", /\b(?:WebSocket|EventSource)\b/g,
    "Browser network streams are v1 APIs.", "Feature-detect this API or use a Node-compatible networking package."],
  ["WEB_MEDIA", "warning", /\b(?:new\s+(?:Image|Audio)\s*\(|AudioContext|webkitAudioContext)\b/g,
    "Image/audio browser APIs are outside the v0 profile.", "Use text and CSS-only assets for v0, or feature-detect the media path."],
];

const CSS_RULES = [
  ["CSS_LAYERS", "error", /(?:^|[;{])\s*(?:visibility|opacity)\s*:/g,
    "visibility/opacity composition is outside the current renderer profile.", "Use conditional DOM rendering instead of hidden composited layers."],
  ["CSS_TRANSFORM", "error", /(?:^|[;{])\s*(?:transform|perspective)\s*:/g,
    "CSS transforms are outside the current renderer profile.", "Express the layout with block, flex, or grid geometry."],
  ["CSS_FIXED", "error", /(?:^|[;{])\s*position\s*:\s*(?:fixed|sticky)\b/g,
    "Fixed and sticky positioning are outside the current renderer profile.", "Use normal, flex, grid, or bounded absolute layout."],
  ["CSS_EFFECT", "error", /(?:^|[;{])\s*(?:filter|backdrop-filter|clip-path|mask(?:-image)?)\s*:/g,
    "This paint effect is outside the current renderer profile.", "Use borders, backgrounds, and static geometry."],
  ["CSS_FONT_FACE", "warning", /@font-face\b/g,
    "Web fonts are a v1 capability.", "Use system fonts for portable v0 output."],
];

const HTML_RULES = [
  ["HTML_CANVAS", "error", /<canvas\b/gi,
    "<canvas> is not implemented.", "Use ordinary DOM/CSS elements or a native viewport."],
  ["HTML_MEDIA", "warning", /<(?:img|picture|audio|video|source)\b/gi,
    "Decoded image and media elements are outside the v0 profile.", "Use CSS colors/text for v0 or feature-detect the media-dependent UI."],
  ["HTML_SVG", "warning", /<svg\b/gi,
    "SVG rendering is currently limited and not in the strict profile.", "Verify this asset visually or replace it with profiled HTML/CSS."],
];

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
  const diagnostics = [];
  for (const file of files) {
    const source = await readFile(file.absolute, "utf8");
    const extension = extname(file.relative).toLowerCase();
    if ([".html", ".htm"].includes(extension)) {
      diagnostics.push(...scanRules(source, file.relative, HTML_RULES));
      diagnostics.push(...scanExternalAssets(source, file.relative, extension));
    } else if (extension === ".css") {
      diagnostics.push(...scanRules(source, file.relative, CSS_RULES));
      diagnostics.push(...scanExternalAssets(source, file.relative, extension));
    } else {
      diagnostics.push(...scanRules(source, file.relative, JS_RULES));
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

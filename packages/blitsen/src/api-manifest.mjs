import { readFile, writeFile } from "node:fs/promises";

const RUNTIME_SOURCE = new URL("../../../crates/blitsen-node/src/dom_bridge.rs", import.meta.url);
const MANIFEST_FILE = new URL("./api-manifest.json", import.meta.url);
const COMPATIBILITY_DOC = new URL("../../../docs/COMPATIBILITY.md", import.meta.url);
const SOURCE_NAME = "crates/blitsen-node/src/dom_bridge.rs";

// The web surface Blitsen makes a claim about, grouped by the diagnostic that
// describes it. Whether an entry is implemented is deliberately not written
// here: it is read out of the runtime source, so the two cannot disagree.
// An entry may carry the pattern `doctor` should match it by, or `null` where
// the name is too ordinary to find in a bundle without false positives.
const CATALOGUE = {
  WEB_DOM: ["document", "Document", "Node", "Element", "NodeList", "DOMTokenList",
    "CSSStyleDeclaration", "MutationObserver", "HTMLElement", "HTMLIFrameElement", "SVGElement",
    "Text", "Comment", "DocumentFragment", "HTMLLinkElement", "HTMLTemplateElement",
    "Element.querySelector", "Element.querySelectorAll", "Element.closest", "Element.matches",
    "Element.cloneNode", "Element.contains", "Element.children", "Element.previousSibling",
    "Element.lastChild", "Element.parentElement", "Element.dataset", "Element.nodeValue",
    "Element.before", "Element.after", "Element.getElementsByTagName", "Element.outerHTML",
    "Element.insertAdjacentHTML", "Element.attachShadow", "Element.scrollIntoView",
    "HTMLLinkElement.relList", "HTMLTemplateElement.content", "DOMTokenList.supports",
    "Document.createElementNS", "Document.createComment", "Document.createDocumentFragment",
    "Document.getElementsByTagName", "Document.importNode"],
  WEB_EVENTS: ["EventTarget", "Event", "CustomEvent", "MouseEvent", "KeyboardEvent",
    "addEventListener", "removeEventListener", "dispatchEvent"],
  WEB_SCHEDULING: ["requestAnimationFrame", "cancelAnimationFrame", "setTimeout", "clearTimeout",
    "setInterval", "clearInterval", "requestIdleCallback", "cancelIdleCallback"],
  WEB_NETWORK: ["fetch", "Headers", "Request", "Response", "Blob", "AbortController",
    "AbortSignal"],
  WEB_ROUTING: ["window", "location", "history", "Location", "History", "PopStateEvent",
    "HashChangeEvent"],
  WEB_VIEWPORT: ["BlitsenViewElement", "BlitsenViewSurface"],
  WEB_STORAGE: ["Storage", "localStorage", "sessionStorage", "indexedDB"],
  WEB_WORKER: ["Worker", "SharedWorker", "ServiceWorker", "ServiceWorkerContainer"],
  WEB_MESSAGING: ["MessageChannel", "MessagePort", "BroadcastChannel", "postMessage"],
  WEB_SOCKET: ["WebSocket", "EventSource"],
  WEB_XHR: ["XMLHttpRequest"],
  WEB_STREAM: ["ReadableStream", "WritableStream", "TransformStream", "Response.body",
    "Response.clone"],
  WEB_FORM: ["FormData", ["File", "\\bnew File\\s*\\("], "FileReader"],
  WEB_CANVAS: ["HTMLCanvasElement", "CanvasRenderingContext2D", "OffscreenCanvas", "ImageData",
    "Path2D"],
  WEB_GPU: ["WebGLRenderingContext", "WebGL2RenderingContext", "GPUCanvasContext"],
  WEB_MEDIA: [["Image", "\\bnew Image\\s*\\("], ["Audio", "\\bnew Audio\\s*\\("], "AudioContext",
    "webkitAudioContext", "HTMLMediaElement", "MediaQueryList", "matchMedia"],
  WEB_DIALOG: [["alert", "\\balert\\s*\\("], ["confirm", "\\bconfirm\\s*\\("],
    ["prompt", "\\bprompt\\s*\\("], ["print", "\\bwindow\\.print\\s*\\("]],
  WEB_NAVIGATION: [["open", "\\bwindow\\.open\\s*\\("], ["close", null], ["navigation", null],
    "document.write", "document.writeln", "document.open", "document.close", "location.assign",
    "location.replace", "location.reload", "location.ancestorOrigins"],
  WEB_COOKIE: ["document.cookie", "cookieStore", "Headers.getSetCookie"],
  WEB_DEVICE: ["Navigator", "navigator", "navigator.userAgent", "navigator.platform",
    "navigator.language", ["screen", null], "Notification", ["caches", null]],
  WEB_OBSERVER: ["ResizeObserver", "IntersectionObserver", "PerformanceObserver"],
  WEB_STYLE: ["getComputedStyle", "CSSStyleSheet", "StyleSheetList"],
  WEB_COMPONENTS: ["customElements", "ShadowRoot", "DOMParser"],
};

// What `doctor` says about a group whose APIs turn out to be absent, plus an
// optional pattern for a usage that names no API at all.
const DIAGNOSTICS = {
  WEB_DOM: ["warning", "This DOM method is not implemented.",
    "Use the document-level lookups and node methods listed in COMPATIBILITY.md."],
  WEB_SCHEDULING: ["warning", "Idle-callback scheduling is not implemented.",
    "Schedule the work with requestAnimationFrame or a timer."],
  WEB_STORAGE: ["error", "IndexedDB is not implemented.",
    "Use a Node filesystem/database package, or the Web Storage APIs for session state."],
  WEB_WORKER: ["error", "Web workers are not implemented.",
    "Run the work in the main context or use a native/Node worker path."],
  WEB_MESSAGING: ["warning", "Message channels are not implemented.",
    "Feature-detect the channel; a scheduler that falls back to a timer keeps working."],
  WEB_SOCKET: ["warning", "Browser network streams are not implemented.",
    "Feature-detect this API or use a Node-compatible networking package."],
  WEB_XHR: ["error", "XMLHttpRequest is not implemented.", "Use fetch with an absolute URL."],
  WEB_STREAM: ["warning", "Streaming bodies are not implemented; a response is buffered whole.",
    "Read the response with text(), json(), or arrayBuffer().", /\.body\s*\.\s*getReader\b/],
  WEB_FORM: ["warning", "Multipart form bodies and file objects are not implemented.",
    "Send a string, Blob, ArrayBuffer, or typed array body."],
  WEB_CANVAS: ["error", "Canvas is not in the v0 compatibility profile.",
    "Use DOM/CSS rendering or a native viewport until canvas support lands.", /\.getContext\s*\(/],
  WEB_GPU: ["error", "WebGL and WebGPU are not implemented.",
    "Remove the GPU-web API path or replace it with a native addon/viewport."],
  WEB_MEDIA: ["warning", "Audio and the media element constructors are not implemented.",
    "Use <img> and CSS assets, or feature-detect the media path."],
  WEB_DIALOG: ["error", "Modal browser dialogs are not implemented.",
    "Use the native dialog module, or render the prompt as DOM."],
  WEB_NAVIGATION: ["error", "Document navigation is deliberately absent; there is no page to leave.",
    "Route with history.pushState and conditional DOM rendering."],
  WEB_COOKIE: ["error", "There is no origin and no cookie jar behind an exported application.",
    "Keep session state in memory or in a file the application owns."],
  WEB_DEVICE: ["warning", "This device API is not implemented.",
    "Feature-detect it, or use the native modules for capability the web does not have."],
  WEB_OBSERVER: ["warning", "Layout and performance observers are not implemented.",
    "Read geometry in a requestAnimationFrame callback, or observe resize on window."],
  WEB_STYLE: ["error", "Computed style and the stylesheet objects are not implemented.",
    "Read the inline style property, or drive the value from a class."],
  WEB_COMPONENTS: ["error", "Custom elements, shadow DOM and DOM parsing are not implemented.",
    "Render with ordinary elements the bundler already emits."],
};

// Diagnostics that are not an absence: an implemented API used in a way an
// exported application cannot honour.
const USAGE_RULES = [
  ["WEB_FETCH", "error", "\\bfetch\\s*\\(\\s*[\"'`](?!https?:\\/\\/)",
    "fetch resolves this URL against an address with no server behind it.",
    "Bundle the data into the export, or request an absolute http(s) URL."],
  // Storage exists and works; what it cannot do is outlive the process, and a
  // write is the only thing that has something to lose by that.
  ["WEB_STORAGE_MEMORY", "warning", "\\blocalStorage\\s*\\.\\s*setItem\\b",
    "localStorage is in memory only: what it stores is gone when the application exits.",
    "Keep anything that must survive a restart in a file the application owns."],
];

// Renderer capability, which no JavaScript declaration describes.
//
// Severity here answers one question: does the page survive? An ignored paint
// property degrades — the page is usable, slightly plainer. A mispositioned box
// can hide content. Only script that throws stops a page rendering at all, and
// none of that is in this list. So nothing here is an error: refusing the build
// leaves the user with nothing, which is strictly worse than the degradation.
//
// These were errors, graded from the S6 capture. The conformance corpus since
// showed that capture was caused by the stale-transition defect (gap G2) rather
// than by the properties blamed for it: `visibility`, `opacity` and `transform`
// all behave correctly, and `paint-suppression.html` gates that. Diagnosing
// working CSS as a build-blocking error refused the stock create-vite template.
const RENDERER_RULES = [
  ["css", "CSS_TRANSITION", "warning", "(?:^|[;{])\\s*transition(?:-property)?\\s*:",
    "A property named by `transition` keeps its pre-stylesheet value (Blitz bug 689).",
    "Inline the rule in a <style> element, or drop the transition, until it is fixed."],
  ["css", "CSS_FIXED", "warning", "(?:^|[;{])\\s*position\\s*:\\s*(?:fixed|sticky)\\b",
    "Fixed and sticky boxes resolve against the root box, not the viewport (Blitz bug 690).",
    "Use normal, flex, grid, or bounded absolute layout for anything that must be placed exactly."],
  ["css", "CSS_EFFECT", "warning",
    "(?:^|[;{])\\s*(?:filter|backdrop-filter|clip-path|mask(?:-image)?)\\s*:",
    "This paint effect is ignored rather than applied.",
    "Check the element is still legible without it; use borders and backgrounds where it is not."],
  ["html", "HTML_CANVAS", "error", "<canvas\\b",
    "<canvas> is not implemented.", "Use ordinary DOM/CSS elements or a native viewport."],
  ["html", "HTML_MEDIA", "warning", "<(?:audio|video|track)\\b",
    "Audio and video elements are not implemented.",
    "Ship the experience as DOM, images and CSS, or feature-detect the media path."],
  ["html", "HTML_SVG", "warning", "<svg\\b",
    "SVG rendering is currently limited and not in the strict profile.",
    "Verify this asset visually or replace it with profiled HTML/CSS."],
];

// Everything below reads the bootstrap as the JavaScript it is, rather than a
// description of it kept alongside.
function bootstrapScript(source) {
  const opening = 'const BOOTSTRAP: &str = r##"';
  const start = source.indexOf(opening);
  const end = source.indexOf('"##;', start);
  if (start < 0 || end < 0) throw new Error(`${SOURCE_NAME} no longer declares a BOOTSTRAP script`);
  return source.slice(start + opening.length, end);
}

// Blanks comments and literal contents while preserving every offset, so a
// structural walk cannot be confused by an apostrophe in a comment.
function blanked(script) {
  const characters = [...script];
  const previous = index => {
    for (let scan = index - 1; scan >= 0; scan--) if (!/\s/.test(script[scan])) return script[scan];
    return "";
  };
  let state = null;
  for (let index = 0; index < characters.length; index++) {
    const character = script[index];
    if (state === null) {
      if (character === "/" && script[index + 1] === "/") state = "line";
      else if (character === "/" && !/[\w$)\]]/.test(previous(index))) state = "regex";
      else if (character === '"' || character === "'" || character === "`") state = character;
      continue;
    }
    if (state === "line") {
      if (character === "\n") state = null;
      else characters[index] = " ";
      continue;
    }
    if (character === "\\") {
      characters[index] = characters[index + 1] = " ";
      index++;
      continue;
    }
    if (character === (state === "regex" ? "/" : state)) state = null;
    else characters[index] = " ";
  }
  return characters.join("");
}

function objectKeys(script, declaration) {
  const start = script.indexOf(declaration);
  if (start < 0) throw new Error(`the bootstrap no longer declares ${declaration}`);
  const keys = [];
  let depth = 0;
  let expectKey = false;
  for (let index = start + declaration.length - 1; index < script.length; index++) {
    const character = script[index];
    if ("{[(".includes(character)) {
      depth++;
      expectKey = depth === 1;
    } else if ("}])".includes(character)) {
      if (--depth === 0) return keys;
    } else if (character === "," && depth === 1) expectKey = true;
    else if (expectKey && /[A-Za-z_$]/.test(character)) {
      const key = /^[\w$]+/.exec(script.slice(index))[0];
      keys.push(key);
      expectKey = false;
      index += key.length - 1;
    }
  }
  throw new Error(`${declaration} is not a closed object literal`);
}

function stringList(script, pattern, name) {
  const match = pattern.exec(script);
  if (!match) throw new Error(`the bootstrap no longer installs ${name} the way this reader parses`);
  return [...match[1].matchAll(/"([^"]+)"/g)].map(([, value]) => value);
}

// Reads the globals, class members and deliberate deletions out of the bootstrap.
export function extractRuntimeSurface(source) {
  const script = bootstrapScript(source);
  const structure = blanked(script);
  const globals = new Set(objectKeys(structure, "const globals = {"));
  for (const [, name] of structure.matchAll(/globalThis\.([A-Za-z_$][\w$]*)\s*=[^=]/g))
    globals.add(name);
  for (const name of stringList(script,
    /for \(const method of (\[[^\]]*\])\)\n\s*Object\.defineProperty\(globalThis, method,/,
    "the global EventTarget methods")) globals.add(name);
  for (const name of stringList(script,
    /for \(const \[name, value\] of (\[\[[\s\S]*?\]\])\)\n\s*Object\.defineProperty\(globalThis, name,/,
    "location and history")) globals.add(name);
  const deleted = new Set(stringList(script,
    /for \(const key of (\[[\s\S]*?\])\) \{\n\s*try \{ delete globalThis\[key\]; \} catch \{\}/,
    "the deliberately absent globals"));

  const classes = new Map();
  for (const [, name, base, body] of structure
    .matchAll(/\n {2}class (\w+)(?: extends (\w+))? \{\n([\s\S]*?)\n {2}\}/g)) {
    const members = [...`\n${body}`.matchAll(/\n {4}(?:static )?(?:get |set )?([A-Za-z_$][\w$]*)\s*[(=]/g)]
      .map(([, member]) => member)
      .filter(member => member !== "constructor");
    classes.set(name, { base, members: new Set(members) });
  }
  const instances = new Map();
  for (const [, name, constructed, created] of structure
    .matchAll(/\n {2}const (\w+) = (?:new (\w+)\(\)|Object\.create\((\w+)\.prototype\));/g))
    if (classes.has(constructed ?? created)) instances.set(name, constructed ?? created);
  return { globals: [...globals].filter(name => !name.startsWith("__blitsen")), classes, instances,
    deleted: [...deleted] };
}

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

// Builds the manifest from the runtime source, refusing anything the two disagree about.
export function buildManifest(source) {
  const surface = extractRuntimeSurface(source);
  const apis = Object.entries(CATALOGUE)
    .flatMap(([code, names]) => names.map(api => apiEntry(surface, api, code)));

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

  return {
    generatedBy: `packages/blitsen/src/api-manifest.mjs from ${SOURCE_NAME}`,
    profile: "v0-strict",
    apis,
    diagnostics: Object.fromEntries(Object.entries(DIAGNOSTICS)
      .map(([code, [severity, message, guidance, extra]]) =>
        [code, { severity, message, guidance, extra: extra?.source ?? null }])),
    usage: USAGE_RULES.map(([code, severity, pattern, message, guidance]) =>
      ({ code, severity, pattern, message, guidance })),
    renderer: RENDERER_RULES.map(([kind, code, severity, pattern, message, guidance]) =>
      ({ kind, code, severity, pattern, message, guidance })),
  };
}

// Loads the generated manifest. The runtime source is not published; this is.
export async function loadApiManifest() {
  return JSON.parse(await readFile(MANIFEST_FILE, "utf8"));
}

export async function generateApiManifest() {
  return buildManifest(await readFile(RUNTIME_SOURCE, "utf8"));
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
  const diagnosed = [
    ...manifest.usage,
    ...codes
      .filter(code => manifest.apis.some(entry => entry.code === code && entry.status === "absent"))
      .map(code => ({ code, ...manifest.diagnostics[code] })),
    ...manifest.renderer,
  ].map(rule => `| \`${rule.code}\` | ${rule.severity} | ${rule.message} |`);
  return ["| Group | Implemented | Absent |", "| --- | --- | --- |", ...surface, "",
    "| Diagnostic | Severity | Reported as |", "| --- | --- | --- |", ...diagnosed].join("\n");
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
  return replaceGenerated(await readFile(COMPATIBILITY_DOC, "utf8"), "api-manifest",
    renderCapabilityTiers(manifest));
}

if (import.meta.main) {
  const manifest = await generateApiManifest();
  await writeFile(MANIFEST_FILE, `${JSON.stringify(manifest, null, 2)}\n`);
  await writeFile(COMPATIBILITY_DOC, await renderCompatibilityDoc(manifest));
  const absent = manifest.apis.filter(entry => entry.status === "absent").length;
  console.log(`api-manifest: ${manifest.apis.length - absent} implemented, ${absent} absent APIs `
    + `read from ${SOURCE_NAME}`);
}

import { readFile, writeFile } from "node:fs/promises";

const RUNTIME_SOURCE_ROOT = "../../../crates/blitsen-host/src/";
const RUNTIME_SOURCE = new URL(`${RUNTIME_SOURCE_ROOT}dom_bridge.rs`, import.meta.url);
const MANIFEST_FILE = new URL("./api-manifest.json", import.meta.url);
const COMPATIBILITY_DOC = new URL("../../../docs/COMPATIBILITY.md", import.meta.url);
const SOURCE_NAME = "crates/blitsen-host/src/dom_bridge.rs";

// The web surface Blitsen makes a claim about, grouped by the diagnostic that
// describes it. Whether an entry is implemented is deliberately not written
// here: it is read out of the runtime source, so the two cannot disagree.
// An entry may carry the pattern `doctor` should match it by, or `null` where
// the name is too ordinary to find in a bundle without false positives.
const CATALOGUE = {
  WEB_DOM: ["document", "Document", "Node", "Element", "NodeList", "DOMTokenList",
    "Attr", "NamedNodeMap",
    "CSSStyleDeclaration", "MutationObserver", "HTMLElement", "HTMLIFrameElement", "SVGElement",
    "Text", "Comment", "DocumentFragment", "HTMLLinkElement", "HTMLTemplateElement",
    "HTMLImageElement", ["Image", "\\bnew Image\\s*\\("],
    "HTMLImageElement.src", "HTMLImageElement.naturalWidth", "HTMLImageElement.naturalHeight",
    "HTMLImageElement.complete", "HTMLImageElement.onload", "HTMLImageElement.onerror",
    "Element.querySelector", "Element.querySelectorAll", "Element.closest", "Element.matches",
    "Element.cloneNode", "Element.contains", "Element.children", "Element.previousSibling",
    "Element.lastChild", "Element.parentElement", "Element.dataset", "Element.nodeValue",
    "Element.before", "Element.after", "Element.getElementsByTagName", "Element.outerHTML",
    "Element.insertAdjacentHTML", "Element.attachShadow", "Element.scrollIntoView",
    "Element.getElementsByClassName", "Element.firstElementChild", "Element.lastElementChild",
    "Element.nextElementSibling", "Element.previousElementSibling", "Element.childElementCount",
    "Element.append", "Element.prepend", "Element.replaceChildren", "Element.getAttributeNS",
    "Element.setAttributeNS", "Element.removeAttributeNS", "Element.hasAttributes",
    "Element.getAttributeNames", "Element.toggleAttribute", "Element.getClientRects",
    "Element.getRootNode", "Element.normalize", "Element.attributes",
    "Element.insertAdjacentElement", "Element.innerText", "Element.compareDocumentPosition",
    "Element.offsetParent", "Element.clientTop", "Element.clientLeft",
    "Element.hidden", "Element.tabIndex", "Element.title",
    "Document.title", "Document.dir", "Document.getElementsByName",
    "Document.elementFromPoint", "Document.elementsFromPoint", "Document.scrollingElement",
    "Document.characterSet", "Document.documentURI", "Document.hasFocus", "Document.adoptNode",
    "HTMLLinkElement.relList", "HTMLTemplateElement.content", "DOMTokenList.supports",
    "Document.createElementNS", "Document.createComment", "Document.createDocumentFragment",
    "Document.getElementsByTagName", "Document.getElementsByClassName", "Document.importNode",
    "Document.currentScript"],
  // The form controls. `value`/`checked` are the control's state and
  // `defaultValue`/`defaultChecked` the attribute reflections; the collections
  // are snapshots, like every other collection here. What stays absent is
  // constraint validation, the label and file lists, text selection, and the
  // navigating half of submission — see COMPATIBILITY.md.
  WEB_FORM_CONTROLS: ["HTMLInputElement", "HTMLTextAreaElement", "HTMLSelectElement",
    "HTMLOptionElement", "HTMLButtonElement", "HTMLFormElement",
    "HTMLInputElement.value", "HTMLInputElement.defaultValue", "HTMLInputElement.checked",
    "HTMLInputElement.defaultChecked", "HTMLInputElement.type", "HTMLInputElement.name",
    "HTMLInputElement.disabled", "HTMLInputElement.form",
    "HTMLInputElement.files", "HTMLInputElement.labels", "HTMLInputElement.validity",
    "HTMLInputElement.checkValidity", "HTMLInputElement.select",
    "HTMLInputElement.setSelectionRange", "HTMLInputElement.selectionStart",
    "HTMLInputElement.selectionEnd",
    "HTMLTextAreaElement.value", "HTMLTextAreaElement.defaultValue",
    "HTMLSelectElement.options", "HTMLSelectElement.selectedIndex", "HTMLSelectElement.value",
    "HTMLSelectElement.length", "HTMLSelectElement.selectedOptions",
    "HTMLSelectElement.multiple", "HTMLSelectElement.add",
    "HTMLOptionElement.value", "HTMLOptionElement.text", "HTMLOptionElement.selected",
    "HTMLOptionElement.index", "HTMLOptionElement.label", "HTMLOptionElement.defaultSelected",
    "HTMLButtonElement.value", "HTMLButtonElement.type",
    "HTMLFormElement.elements", "HTMLFormElement.requestSubmit", "HTMLFormElement.submit",
    "HTMLFormElement.reset", "HTMLFormElement.action", "HTMLFormElement.method",
    "HTMLFormElement.checkValidity"],
  WEB_EVENTS: ["EventTarget", "Event", "CustomEvent", "SubmitEvent", "MouseEvent",
    "KeyboardEvent", "FocusEvent", "InputEvent", "PointerEvent", "WheelEvent",
    "addEventListener", "removeEventListener", "dispatchEvent"],
  // Document scrolling. `scroll` and `scrollTo` are the same function under two
  // names, as they are on Window. The patterns are qualified because the bare
  // words are far too ordinary to find in a bundle: `scroll` alone matches every
  // scroll listener, class name and CSS property in the file.
  WEB_SCROLL: [["scrollTo", "\\bwindow\\.scrollTo\\s*\\("],
    ["scrollBy", "\\bwindow\\.scrollBy\\s*\\("], ["scroll", "\\bwindow\\.scroll\\s*\\("],
    ["scrollX", "\\b(?:window|globalThis)\\.scrollX\\b"],
    ["scrollY", "\\b(?:window|globalThis)\\.scrollY\\b"],
    ["pageXOffset", "\\bpageXOffset\\b"], ["pageYOffset", "\\bpageYOffset\\b"]],
  // Selection and ranges, absent together: a caller that has a selection wants
  // the ranges in it, so implementing either alone would answer half a question.
  WEB_SELECTION: [["getSelection", "\\b(?:window|document)\\.getSelection\\s*\\("], "Range"],
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
  // `MessageEvent` is here rather than with the messaging APIs because a socket
  // is the only thing in this runtime that delivers one. The four readyState
  // constants are not listed: they are installed onto the constructor and the
  // prototype rather than declared in the class body, which is not a shape this
  // file's reader can verify, and `readyState` covers what a bundle reads.
  WEB_SOCKET: ["WebSocket", "MessageEvent", "CloseEvent", "EventSource",
    "WebSocket.url", "WebSocket.readyState", "WebSocket.protocol", "WebSocket.extensions",
    "WebSocket.bufferedAmount", "WebSocket.binaryType", "WebSocket.send", "WebSocket.close"],
  WEB_XHR: ["XMLHttpRequest"],
  WEB_STREAM: ["ReadableStream", "WritableStream", "TransformStream", "Response.body",
    "Response.clone"],
  WEB_FORM: ["FormData", ["File", "\\bnew File\\s*\\("], "FileReader"],
  WEB_CANVAS: ["HTMLCanvasElement", "CanvasRenderingContext2D", "OffscreenCanvas", "ImageData",
    "Path2D"],
  WEB_GPU: ["WebGLRenderingContext", "WebGL2RenderingContext", "GPUCanvasContext"],
  // Audio, at the size the issue asked for: a context, gain, stereo panning and
  // buffer sources, plus the element built on them. The rest of Web Audio —
  // filters, oscillators, analysers, convolution, worklets, the HRTF panner —
  // is absent rather than half-built, and is not listed here because listing an
  // API means making a claim about it. COMPATIBILITY.md names what is missing.
  WEB_MEDIA: [["Audio", "\\bnew Audio\\s*\\("], "AudioContext", "AudioNode", "AudioParam",
    "AudioBuffer", "AudioBufferSourceNode", "AudioDestinationNode", "GainNode",
    "StereoPannerNode", "HTMLAudioElement",
    "AudioContext.decodeAudioData", "AudioContext.createGain", "AudioContext.createStereoPanner",
    "AudioContext.createBufferSource", "AudioContext.destination", "AudioContext.currentTime",
    "AudioContext.sampleRate", "AudioContext.resume", "AudioContext.suspend",
    "AudioContext.close",
    "webkitAudioContext", "HTMLMediaElement"],
  WEB_DIALOG: [["alert", "\\balert\\s*\\("], ["confirm", "\\bconfirm\\s*\\("],
    ["prompt", "\\bprompt\\s*\\("], ["print", "\\bwindow\\.print\\s*\\("]],
  // `stop` belongs here rather than with the network APIs: the spec defines it
  // on Window alongside navigation, and what it aborts is the document's load
  // rather than a request the application made.
  WEB_NAVIGATION: [["stop", "\\bwindow\\.stop\\s*\\("],
    ["open", "\\bwindow\\.open\\s*\\("], ["close", null], ["navigation", null],
    "document.write", "document.writeln", "document.open", "document.close", "location.assign",
    "location.replace", "location.reload", "location.ancestorOrigins"],
  WEB_COOKIE: ["document.cookie", "cookieStore", "Headers.getSetCookie"],
  WEB_DEVICE: ["Navigator", "navigator", "navigator.userAgent", "navigator.platform",
    "navigator.language", ["screen", null], "Notification", ["caches", null]],
  WEB_OBSERVER: ["ResizeObserver", "IntersectionObserver", "PerformanceObserver"],
  // The CSSOM stylesheet objects, at the size a framework transition needs: a
  // sheet is the `<style>` element that owns it, and a rule inserted into one
  // reaches the same cascade the document's own stylesheets do. What stays
  // absent is the rest of CSSOM — the rule subclasses, a rule's declarations and
  // selector, constructible and adopted sheets, and `disabled`.
  WEB_STYLE: ["getComputedStyle", "matchMedia", "MediaQueryList", "MediaQueryListEvent",
    ["CSS", "\\bCSS\\.(?:escape|supports)\\s*\\("],
    "CSSStyleSheet", "StyleSheetList", "CSSRule", "CSSRuleList", "HTMLStyleElement",
    "CSSStyleRule", "CSSKeyframesRule", "CSSKeyframeRule", "CSSMediaRule",
    "document.styleSheets", "document.adoptedStyleSheets",
    "HTMLStyleElement.sheet", "HTMLLinkElement.sheet",
    "CSSStyleSheet.cssRules", "CSSStyleSheet.insertRule", "CSSStyleSheet.deleteRule",
    "CSSStyleSheet.ownerNode", "CSSStyleSheet.href", "CSSStyleSheet.title",
    "CSSStyleSheet.disabled", "CSSStyleSheet.replaceSync", "CSSStyleSheet.replace",
    "CSSRule.cssText", "CSSRule.parentStyleSheet", "CSSRule.style", "CSSRule.selectorText",
    "CSSRule.type"],
  WEB_COMPONENTS: ["customElements", "ShadowRoot", "DOMParser"],
};

// The `native:` modules, declared the same way and for the same reason: the
// names live here and whether each one is implemented is read out of the
// bootstrap, so this file cannot claim capability the runtime does not install.
// Nothing here has a Node or web spelling — that is the entry condition for a
// `native:` member (TECH.md §9), which is why `argv`, `execPath` and `quit` are
// not listed as absent: they are `process.argv`, `process.execPath` and
// `process.exit`, and they are not this layer's to name.
// Note what `window` does not name: size, position and scale factor. Those are
// `innerWidth`, `innerHeight` and `devicePixelRatio`, and the `resize` event
// says when they changed — the additive rule applies to the web surface as well
// as to Node's, and a second answer that could disagree with those is worse than
// no answer. Per-monitor DPI is not the same fact and is in `monitors`.
const NATIVE = {
  app: ["dataDir", "cacheDir", "configDir", "requestSingleInstanceLock", "relaunch",
    "onQuitRequest", "onSuspend", "onResume", "registerProtocol", "registerFileAssociation"],
  window: ["setSize", "setFullscreen", "isFullscreen", "setDecorations", "isDecorated",
    "setAlwaysOnTop", "setCursor", "setCursorVisible", "setCursorGrab", "monitors",
    "create", "setTransparent", "isAlwaysOnTop"],
  dialog: ["openFile", "openFiles", "saveFile", "openFolder", "openFolders", "message"],
  clipboard: ["readText", "readHtml", "readImage", "writeText", "writeHtml", "writeImage",
    "clear", "readMime", "writeMime"],
};

// Why a declared member is not implemented. Absence is the answer, not an
// oversight: the member is `undefined`, so `if (app.onQuitRequest)` selects a
// fallback, and this is what the documentation says about each one.
const NATIVE_ABSENT = {
  "app.onQuitRequest": "A close request is a window event, and windows are issue #77's to expose; "
    + "delivering one from here would mean a second, competing event loop.",
  "app.onSuspend": "Linux has no process-level suspend notification to report. The desktop "
    + "portals that come closest describe the session, not this application.",
  "app.onResume": "The counterpart of `onSuspend`, absent for the same reason.",
  "app.registerProtocol": "Registering `myapp://` on Linux means installing a `.desktop` entry "
    + "that names the executable, which is what `blitsen build` already writes. A running process "
    + "editing that entry would fight its own packaging. The activation itself arrives: the "
    + "desktop launches the handler with the URL in `argv`, and the single-instance lock hands "
    + "that to the instance already running.",
  "app.registerFileAssociation": "The same `.desktop` entry, with `MimeType` instead of a scheme.",
  "window.create": "A second window needs the shared-versus-isolated JavaScript context question "
    + "answered first: whether two windows see one `document` and one module graph or two decides "
    + "what `create` even returns, and it cannot be settled by implementing it. The window this "
    + "run already opened is what the rest of this module operates on.",
  "window.setTransparent": "Transparency is chosen when a window is created — winit's own setter "
    + "does nothing on X11 after that — so honouring it would mean replacing the window, which is "
    + "`create`. Run `blitsen` against a directory whose window should be transparent and the "
    + "attribute belongs on that window, not on a call.",
  "window.isAlwaysOnTop": "winit sets the window level and cannot read it back, and the window "
    + "manager may change it without telling the application. Remembering what was last set would "
    + "be a second source of truth that quietly goes stale.",
  "clipboard.readMime": "`arboard` reads the flavours above and no others. Arbitrary MIME needs a "
    + "different mechanism on each platform — X11 selection targets, `wl_data_offer`, "
    + "`NSPasteboardType`, a registered Windows format — and no part of that is shared.",
  "clipboard.writeMime": "The counterpart of `readMime`, absent for the same reason.",
};

// What `doctor` says about a group whose APIs turn out to be absent, plus an
// optional pattern for a usage that names no API at all.
//
// Every entry here is a warning, and that is a decision rather than an omission.
// What takes a page down is an *unguarded* reference to an absent global; a
// guarded one selects a fallback and the page survives. This scan sees
// references, not guards, and the references it finds in real bundles are
// overwhelmingly guarded: `typeof XMLHttpRequest<"u"`, `typeof ShadowRoot<"u"`,
// `"serviceWorker" in navigator`, a try/catch around `document.cookie`. Measured
// on unmodified third-party builds: shadcn-admin carried 19 of these and renders
// its whole admin shell, 364 elements in 16 colours; vue3-realworld carried 5 and
// renders. Blocking those builds was the diagnostic being wrong, loudly, and
// pointing at an override that does not exist.
//
// Detecting the guard was the alternative. It is not reliable — the guard is
// arbitrary minified JavaScript and may be several frames from the reference —
// and a half-working detector would trade these false errors for false silence
// on the unguarded reference that does kill a page. So the finding is still
// reported, at the severity a finding this imprecise is worth. The field stays
// declared per group: the day an absence is fatal however it is written, it is
// graded here rather than around here.
const DIAGNOSTICS = {
  WEB_DOM: ["warning", "This DOM method is not implemented.",
    "Use the document-level lookups and node methods listed in COMPATIBILITY.md."],
  WEB_FORM_CONTROLS: ["warning", "This form-control API is not implemented.",
    "Validate and select in application code; handle the cancelable submit event rather than "
    + "submitting the form."],
  WEB_SCHEDULING: ["warning", "Idle-callback scheduling is not implemented.",
    "Schedule the work with requestAnimationFrame or a timer."],
  WEB_STORAGE: ["warning", "IndexedDB is not implemented.",
    "Use a Node filesystem/database package, or the Web Storage APIs for session state."],
  WEB_WORKER: ["warning", "Web workers are not implemented.",
    "Run the work in the main context or use a native/Node worker path."],
  WEB_MESSAGING: ["warning", "Message channels are not implemented.",
    "Feature-detect the channel; a scheduler that falls back to a timer keeps working."],
  WEB_SOCKET: ["warning", "Server-sent events are not implemented; WebSocket is.",
    "Feature-detect EventSource, or hold the stream open over a WebSocket instead."],
  WEB_XHR: ["warning", "XMLHttpRequest is not implemented.", "Use fetch with an absolute URL."],
  WEB_STREAM: ["warning", "Streaming bodies are not implemented; a response is buffered whole.",
    "Read the response with text(), json(), or arrayBuffer().", /\.body\s*\.\s*getReader\b/],
  WEB_FORM: ["warning", "Multipart form bodies and file objects are not implemented.",
    "Send a string, Blob, ArrayBuffer, or typed array body."],
  WEB_CANVAS: ["warning", "Canvas is not in the v0 compatibility profile.",
    "Use DOM/CSS rendering or a native viewport until canvas support lands.", /\.getContext\s*\(/],
  WEB_GPU: ["warning", "WebGL and WebGPU are not implemented.",
    "Remove the GPU-web API path or replace it with a native addon/viewport."],
  WEB_MEDIA: ["warning", "This media API is not implemented; Web Audio and <audio> are.",
    "Decode with AudioContext.decodeAudioData and play through a buffer source, or "
    + "feature-detect the media path."],
  WEB_DIALOG: ["warning", "Modal browser dialogs are not implemented.",
    "Use the native dialog module, or render the prompt as DOM."],
  WEB_NAVIGATION: ["warning",
    "Document navigation is deliberately absent; there is no page to leave.",
    "Route with history.pushState and conditional DOM rendering."],
  WEB_COOKIE: ["warning", "There is no origin and no cookie jar behind an exported application.",
    "Keep session state in memory or in a file the application owns."],
  WEB_DEVICE: ["warning", "This device API is not implemented.",
    "Feature-detect it, or use the native modules for capability the web does not have."],
  WEB_OBSERVER: ["warning", "This observer is not implemented; only ResizeObserver is.",
    "Read geometry in a requestAnimationFrame callback, or observe the element's size instead."],
  WEB_STYLE: ["warning",
    "This part of CSSOM is not implemented; a sheet's rules are its source text.",
    "Insert or delete a whole rule through the sheet of a <style> element, and read values "
    + "back with getComputedStyle."],
  WEB_COMPONENTS: ["warning", "Custom elements and shadow DOM are not implemented; DOMParser is.",
    "Render with ordinary elements the bundler already emits."],
  WEB_SELECTION: ["warning", "Text selection and ranges are not implemented.",
    "Track the selected range in application state rather than reading it back from the DOM."],
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

// Subresources an exported application cannot fetch: there is no server behind
// the document, and the renderer serves local files only.
//
// Severity is the same survival question, and every one of these degrades. The
// renderer answers a request it will not serve with empty bytes rather than
// dropping it, so a refused stylesheet, font or image leaves the page standing:
// shadcn-admin renders its whole admin shell with three remote links refused,
// Google Fonts among them, and vue3-realworld renders with two.
//
// A remote `<script src>` used to be the exception, because the loader aborted
// the whole run on one — which is what stopped wordle-plus loading. It no longer
// does: `blitsen-core`'s script loader skips that one script, says so on stderr,
// and runs the rest of the page. So the reason this was ever graded an error is
// gone, and grading it one now only blocks a build that would have worked. What
// keeps an exported application from silently phoning home is the runtime
// refusing to fetch the script, not the severity of this rule.
const REMOTE_ASSET = [
  "A remote asset is not part of a self-contained export; the request is answered with nothing.",
  "Bundle the asset into the output directory and reference its local path, and check the page "
  + "still reads without it.",
];
const ASSET_RULES = [
  ["html", "ASSET_REMOTE_SCRIPT", "warning",
    "<script\\b[^>]*\\bsrc\\s*=\\s*[\"'](?:https?:)?//",
    "A remote <script src> is not fetched; it is skipped and the rest of the page runs.",
    "Bundle the script into the output directory and reference its local path."],
  ["html", "ASSET_REMOTE", "warning",
    "<(?:img|source|audio|video|track|embed|input)\\b[^>]*\\bsrc\\s*=\\s*[\"'](?:https?:)?//"
    + "|<link\\b[^>]*\\bhref\\s*=\\s*[\"'](?:https?:)?//"
    + "|<video\\b[^>]*\\bposter\\s*=\\s*[\"'](?:https?:)?//"
    + "|<object\\b[^>]*\\bdata\\s*=\\s*[\"'](?:https?:)?//", ...REMOTE_ASSET],
  ["css", "ASSET_REMOTE", "warning", "url\\(\\s*[\"']?(?:https?:)?//", ...REMOTE_ASSET],
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
  ["html", "HTML_MEDIA", "warning", "<(?:video|track)\\b",
    "Video and text tracks are not implemented; <audio> is.",
    "Ship moving pictures as DOM, images and CSS, or feature-detect the media path."],
  ["html", "HTML_SVG", "warning", "<svg\\b",
    "SVG rendering is currently limited and not in the strict profile.",
    "Verify this asset visually or replace it with profiled HTML/CSS."],
];

// Everything below reads the bootstrap as the JavaScript it is, rather than a
// description of it kept alongside.
//
// The script is spliced together from `dom_bridge/bootstrap/*.js`, and the
// splice order lives in the Rust that evaluates it. Reading the order from
// there rather than restating it keeps the manifest describing the same script
// the runtime actually runs, and turns a renamed fragment into a loud failure.
export async function readBootstrapScript() {
  const rust = await readFile(RUNTIME_SOURCE, "utf8");
  const fragments = [...rust.matchAll(/include_str!\("(dom_bridge\/bootstrap\/[^"]+)"\)/g)]
    .map(([, path]) => new URL(RUNTIME_SOURCE_ROOT + path, import.meta.url));
  if (fragments.length === 0)
    throw new Error(`${SOURCE_NAME} no longer splices a bootstrap script`);
  return (await Promise.all(fragments.map(file => readFile(file, "utf8")))).join("");
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
export function extractRuntimeSurface(script) {
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
  // The document's scroll offsets, which are accessors rather than values and
  // so are not in the `globals` object literal. Only the first name of each
  // pair is a global; the second is the element property it reads.
  for (const [index, name] of stringList(script,
    /for \(const \[name, axis\] of (\[\[[\s\S]*?\]\])\)\n\s*Object\.defineProperty\(globalThis, name,/,
    "the scroll offsets").entries()) if (index % 2 === 0) globals.add(name);
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
  const native = new Map(Object.keys(NATIVE).map(module =>
    [module, new Set(objectKeys(structure, `const native${capitalized(module)} = {`))]));
  return { globals: [...globals].filter(name => !name.startsWith("__blitsen")), classes, instances,
    deleted: [...deleted], native };
}

const capitalized = name => `${name[0].toUpperCase()}${name.slice(1)}`;

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
      return { api, module, member, status, ...(reason ? { reason } : {}) };
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

// The published type definitions, checked against the manifest rather than
// maintained beside it (issue #74).
//
// The failure this prevents is the one that costs a user the most: editor
// completion offering `native:window.create`, the code compiling, and the call
// returning `undefined` at run time. So the rule is exact in both directions —
// a declared member the runtime does not install is a promise, and an installed
// member left undeclared is completion the user does not get.
//
// Each `blitsen/<module>` subpath has its own declaration file. The interface
// it names carries the members; a module with none names `NativeUnimplemented`,
// which declares none, so the check reads an empty set for it and the two agree.
const TYPE_DEFINITIONS = new URL("./native/native.d.ts", import.meta.url);
const MODULE_INTERFACES = { app: "NativeApp", window: "NativeWindow",
  dialog: "NativeDialog", clipboard: "NativeClipboard" };

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
    "| Diagnostic | Severity | Reported as |", "| --- | --- | --- |", ...diagnosed].join("\n");
}

// Renders the `native:` module surface documented in COMPATIBILITY.md.
export function renderNativeModules(manifest) {
  const modules = [...new Set(manifest.native.map(entry => entry.module))];
  const members = (module, status) => manifest.native
    .filter(entry => entry.module === module && entry.status === status)
    .map(entry => `\`${entry.member}\``).join(", ") || "—";
  const surface = modules.map(module => `| \`blitsen/${module}\` `
    + `| ${members(module, "implemented")} | ${members(module, "absent")} |`);
  const absent = manifest.native.filter(entry => entry.status === "absent")
    .map(entry => `| \`${entry.api}\` | ${entry.reason} |`);
  return ["| Module | Implemented | Absent |", "| --- | --- | --- |", ...surface, "",
    "| Absent member | Why |", "| --- | --- |", ...absent].join("\n");
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

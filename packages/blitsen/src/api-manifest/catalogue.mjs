import {
  REMOTE_CSS_ASSET_PATTERN, REMOTE_HTML_ASSET_PATTERN, REMOTE_HTML_SCRIPT_PATTERN,
} from "../asset-references.mjs";

// The web surface Blitsen makes a claim about, grouped by the diagnostic that
// describes it. Whether an entry is implemented is deliberately not written
// here: it is read out of the runtime source, so the two cannot disagree.
// An entry may carry the pattern `doctor` should match it by, or `null` where
// the name is too ordinary to find in a bundle without false positives.
export const CATALOGUE = {
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
    "HTMLLinkElement.relList", "HTMLLinkElement.onload", "HTMLLinkElement.onerror",
    "HTMLTemplateElement.content", "DOMTokenList.supports",
    "Document.createElementNS", "Document.createComment", "Document.createDocumentFragment",
    "Document.getElementsByTagName", "Document.getElementsByClassName", "Document.importNode",
    "Document.currentScript",
    // `entries`, `keys` and `values` are implemented and unlisted: they are
    // generator methods, which this file's reader cannot see in the source, and
    // listing one would be a claim it could not check.
    "NodeList.item", "NodeList.forEach"],
  // The form controls. `value`/`checked` are the control's state and
  // `defaultValue`/`defaultChecked` the attribute reflections; the collections
  // are snapshots, like every other collection here. The selection members
  // answer for `<textarea>` and the single-line-text input types and are null
  // on the rest, which is HTML's own rule. What stays absent is constraint
  // validation, the label and file lists, and the navigating half of
  // submission — see COMPATIBILITY.md.
  WEB_FORM_CONTROLS: ["HTMLInputElement", "HTMLTextAreaElement", "HTMLSelectElement",
    "HTMLOptionElement", "HTMLButtonElement", "HTMLFormElement",
    "HTMLInputElement.value", "HTMLInputElement.defaultValue", "HTMLInputElement.checked",
    "HTMLInputElement.defaultChecked", "HTMLInputElement.type", "HTMLInputElement.name",
    "HTMLInputElement.disabled", "HTMLInputElement.form",
    "HTMLInputElement.files", "HTMLInputElement.labels", "HTMLInputElement.validity",
    "HTMLInputElement.checkValidity", "HTMLInputElement.select",
    "HTMLInputElement.setSelectionRange", "HTMLInputElement.selectionStart",
    "HTMLInputElement.selectionEnd", "HTMLInputElement.selectionDirection",
    "HTMLTextAreaElement.value", "HTMLTextAreaElement.defaultValue",
    "HTMLTextAreaElement.select", "HTMLTextAreaElement.setSelectionRange",
    "HTMLTextAreaElement.selectionStart", "HTMLTextAreaElement.selectionEnd",
    "HTMLTextAreaElement.selectionDirection",
    "HTMLSelectElement.options", "HTMLSelectElement.selectedIndex", "HTMLSelectElement.value",
    "HTMLSelectElement.length", "HTMLSelectElement.selectedOptions",
    "HTMLSelectElement.multiple", "HTMLSelectElement.add",
    "HTMLOptionElement.value", "HTMLOptionElement.text", "HTMLOptionElement.selected",
    "HTMLOptionElement.index", "HTMLOptionElement.label", "HTMLOptionElement.defaultSelected",
    "HTMLButtonElement.value", "HTMLButtonElement.type",
    "HTMLFormElement.elements", "HTMLFormElement.requestSubmit", "HTMLFormElement.submit",
    "HTMLFormElement.reset", "HTMLFormElement.action", "HTMLFormElement.method",
    "HTMLFormElement.checkValidity"],
  // Pointer capture is declared here rather than with the rest of `Element`
  // because it is the pointer-event surface: it means nothing without a
  // `pointerId`, and an application that reaches for one reaches for both.
  WEB_EVENTS: ["EventTarget", "Event", "CustomEvent", "SubmitEvent", "MouseEvent",
    "KeyboardEvent", "FocusEvent", "InputEvent", "CompositionEvent", "PointerEvent", "WheelEvent",
    "addEventListener", "removeEventListener", "dispatchEvent", "ErrorEvent",
    "Element.setPointerCapture", "Element.releasePointerCapture",
    "Element.hasPointerCapture", "Element.requestPointerLock", "Element.requestFullscreen",
    "Document.pointerLockElement", "Document.exitPointerLock",
    "Document.fullscreenElement", "Document.fullscreenEnabled", "Document.exitFullscreen"],
  // Clipboard events and drag and drop (issue #93), which are one group because
  // they are one object: a `DataTransfer` reaches an application either as
  // `clipboardData` or as `dataTransfer` and behaves the same in both.
  //
  // What is absent is the file half, and deliberately: `files` and `items` hand
  // back `File` objects, which this runtime does not have and does not want —
  // a drop reports the absolute filesystem paths the platform gave, in `paths`,
  // which is the divergence PRODUCT.md §7 argues for. The patterns are the
  // dotted spelling a bundle really writes, so an application reading `files`
  // off a drop is told what to read instead. `setDragImage` belongs to starting
  // a drag, which winit gives no way to do.
  WEB_TRANSFER: ["ClipboardEvent", "DragEvent", "DataTransfer",
    "DataTransfer.dropEffect", "DataTransfer.effectAllowed", "DataTransfer.types",
    "DataTransfer.getData", "DataTransfer.setData", "DataTransfer.clearData",
    "DataTransfer.paths",
    ["DataTransfer.files", "\\bdataTransfer\\s*\\.\\s*files\\b"],
    ["DataTransfer.items", "\\bdataTransfer\\s*\\.\\s*items\\b"],
    ["DataTransfer.setDragImage", "\\bsetDragImage\\s*\\("]],
  // Document scrolling. `scroll` and `scrollTo` are the same function under two
  // names, as they are on Window. The patterns are qualified because the bare
  // words are far too ordinary to find in a bundle: `scroll` alone matches every
  // scroll listener, class name and CSS property in the file.
  WEB_SCROLL: [["scrollTo", "\\bwindow\\.scrollTo\\s*\\("],
    ["scrollBy", "\\bwindow\\.scrollBy\\s*\\("], ["scroll", "\\bwindow\\.scroll\\s*\\("],
    ["scrollX", "\\b(?:window|globalThis)\\.scrollX\\b"],
    ["scrollY", "\\b(?:window|globalThis)\\.scrollY\\b"],
    ["pageXOffset", "\\bpageXOffset\\b"], ["pageYOffset", "\\bpageYOffset\\b"]],
  // Selection and ranges. Geometry is why they are here: measuring a run of
  // characters means putting a range around it and asking where it is, and
  // nothing else in the DOM can answer that. A selection is declared alongside
  // because a caller that has one wants the ranges in it.
  //
  // What stays absent is every method that edits the tree through a range —
  // `deleteContents`, `extractContents`, `cloneContents`, `insertNode`,
  // `surroundContents` — and the reason is in COMPATIBILITY.md: each one splits
  // a text node at a boundary point, and this runtime has no character-data
  // interface to split one with.
  WEB_SELECTION: [["getSelection", "\\b(?:window|document)\\.getSelection\\s*\\("], "Range",
    "Selection", "CaretPosition",
    "Document.createRange", "Document.getSelection", "Document.caretRangeFromPoint",
    "Document.caretPositionFromPoint",
    "Range.setStart", "Range.setEnd", "Range.setStartBefore", "Range.setStartAfter",
    "Range.setEndBefore", "Range.setEndAfter", "Range.selectNode", "Range.selectNodeContents",
    "Range.collapse", "Range.cloneRange", "Range.startContainer", "Range.startOffset",
    "Range.endContainer", "Range.endOffset", "Range.collapsed",
    "Range.commonAncestorContainer", "Range.comparePoint", "Range.compareBoundaryPoints",
    "Range.intersectsNode", "Range.isPointInRange", "Range.toString",
    "Range.getClientRects", "Range.getBoundingClientRect",
    "Range.deleteContents", "Range.extractContents", "Range.cloneContents", "Range.insertNode",
    "Range.surroundContents",
    "Selection.anchorNode", "Selection.anchorOffset", "Selection.focusNode",
    "Selection.focusOffset", "Selection.isCollapsed", "Selection.rangeCount", "Selection.type",
    "Selection.direction", "Selection.getRangeAt", "Selection.addRange",
    "Selection.removeAllRanges", "Selection.setBaseAndExtent", "Selection.collapse",
    "Selection.extend", "Selection.selectAllChildren", "Selection.containsNode",
    "Selection.toString",
    "CaretPosition.offsetNode", "CaretPosition.offset", "CaretPosition.getClientRect"],
  WEB_SCHEDULING: ["requestAnimationFrame", "cancelAnimationFrame", "setTimeout", "clearTimeout",
    "setInterval", "clearInterval", "requestIdleCallback", "cancelIdleCallback"],
  WEB_NETWORK: ["fetch", "Headers", "Request", "Response", "Blob", "AbortController",
    "AbortSignal"],
  // WHATWG URL, over the same Rust parser `location` reads. Object URLs are the
  // absent half: a `blob:` URL is a handle into a store a later fetch reads, and
  // there is no origin behind an application to hang one on.
  // `canParse` and `parse` are implemented and deliberately unlisted: a member
  // is looked up on the prototype, and a static one is not there — listing it
  // would be a claim this file cannot check against the runtime.
  WEB_URL: ["URL", "URLSearchParams",
    ["URL.createObjectURL", "\\bURL\\.createObjectURL\\s*\\("],
    ["URL.revokeObjectURL", "\\bURL\\.revokeObjectURL\\s*\\("]],
  WEB_ROUTING: ["window", ["self", "\\bself\\."], "location", "history", "Location", "History", "PopStateEvent",
    "HashChangeEvent"],
  WEB_VIEWPORT: ["BlitsenViewElement", "BlitsenViewSurface"],
  WEB_STORAGE: ["Storage", "localStorage", "sessionStorage", "indexedDB"],
  // A worker is a second JavaScript context on a thread of its own, with the
  // same application behind it and no DOM in front of it. `SharedWorker` and
  // the service worker family stay absent: both are about sharing one worker
  // between documents, and there is one document.
  WEB_WORKER: ["Worker", "Worker.postMessage", "Worker.terminate",
    "SharedWorker", "ServiceWorker", "ServiceWorkerContainer"],
  WEB_MESSAGING: ["MessageChannel", "MessagePort", "structuredClone",
    ["postMessage", "\\b(?:window|globalThis|self)\\.postMessage\\s*\\("],
    "MessagePort.postMessage", "MessagePort.start", "MessagePort.close",
    "BroadcastChannel"],
  // `MessageEvent` is here rather than with the messaging APIs because the two
  // transports below are the only things in this runtime that deliver one — a
  // socket frame and a server-sent event both arrive as one. The readyState
  // constants are not listed: they are installed onto the constructor and the
  // prototype rather than declared in the class body, which is not a shape this
  // file's reader can verify, and `readyState` covers what a bundle reads.
  WEB_SOCKET: ["WebSocket", "MessageEvent", "CloseEvent", "EventSource",
    "WebSocket.url", "WebSocket.readyState", "WebSocket.protocol", "WebSocket.extensions",
    "WebSocket.bufferedAmount", "WebSocket.binaryType", "WebSocket.send", "WebSocket.close",
    "EventSource.url", "EventSource.readyState", "EventSource.withCredentials",
    "EventSource.close"],
  // `Intl` is the bridge's now rather than the engine's (#237): the formatters
  // are native, over CLDR through ICU4X and the platform's own time-zone
  // database. What each of them can and cannot do is declared in DECLARED
  // below, because the classes live inside the object rather than beside it.
  WEB_INTL: ["Intl"],
  WEB_XHR: ["XMLHttpRequest"],
  WEB_STREAM: ["ReadableStream", "WritableStream", "TransformStream", "Response.body",
    "Response.clone"],
  WEB_FORM: ["FormData", ["File", "\\bnew File\\s*\\("], "FileReader"],
  // The 2D context (issue #99). What is listed is what the bootstrap installs,
  // and what is not is as deliberate: shadows and `filter` both need a blur,
  // and nothing under this renderer has one — the same reason `doctor` reports
  // CSS `filter` as ignored. `OffscreenCanvas` and `ImageBitmap` are a second
  // rendering target and a second decode path rather than more of this one.
  //
  // `DOMMatrix` is here rather than in a geometry group of its own because it
  // exists for `getTransform`: nothing else in this runtime returns one.
  WEB_CANVAS: ["HTMLCanvasElement", "CanvasRenderingContext2D", "ImageData", "Path2D",
    "CanvasGradient", "CanvasPattern", "TextMetrics", "DOMMatrix",
    "OffscreenCanvas", "OffscreenCanvasRenderingContext2D", "ImageBitmap",
    ["createImageBitmap", "\\bcreateImageBitmap\\s*\\("],
    "HTMLCanvasElement.width", "HTMLCanvasElement.height", "HTMLCanvasElement.getContext",
    "HTMLCanvasElement.toDataURL", "HTMLCanvasElement.toBlob",
    "HTMLCanvasElement.captureStream", "HTMLCanvasElement.transferControlToOffscreen",
    "CanvasRenderingContext2D.canvas", "CanvasRenderingContext2D.save",
    "CanvasRenderingContext2D.restore", "CanvasRenderingContext2D.reset",
    "CanvasRenderingContext2D.scale", "CanvasRenderingContext2D.rotate",
    "CanvasRenderingContext2D.translate", "CanvasRenderingContext2D.transform",
    "CanvasRenderingContext2D.setTransform", "CanvasRenderingContext2D.resetTransform",
    "CanvasRenderingContext2D.getTransform", "CanvasRenderingContext2D.globalAlpha",
    "CanvasRenderingContext2D.globalCompositeOperation",
    "CanvasRenderingContext2D.fillStyle", "CanvasRenderingContext2D.strokeStyle",
    "CanvasRenderingContext2D.lineWidth", "CanvasRenderingContext2D.lineCap",
    "CanvasRenderingContext2D.lineJoin", "CanvasRenderingContext2D.miterLimit",
    "CanvasRenderingContext2D.setLineDash", "CanvasRenderingContext2D.getLineDash",
    "CanvasRenderingContext2D.lineDashOffset", "CanvasRenderingContext2D.font",
    "CanvasRenderingContext2D.textAlign", "CanvasRenderingContext2D.textBaseline",
    "CanvasRenderingContext2D.direction",
    "CanvasRenderingContext2D.imageSmoothingEnabled",
    "CanvasRenderingContext2D.imageSmoothingQuality",
    "CanvasRenderingContext2D.beginPath", "CanvasRenderingContext2D.closePath",
    "CanvasRenderingContext2D.moveTo", "CanvasRenderingContext2D.lineTo",
    "CanvasRenderingContext2D.quadraticCurveTo", "CanvasRenderingContext2D.bezierCurveTo",
    "CanvasRenderingContext2D.arc", "CanvasRenderingContext2D.arcTo",
    "CanvasRenderingContext2D.ellipse", "CanvasRenderingContext2D.rect",
    "CanvasRenderingContext2D.roundRect", "CanvasRenderingContext2D.fill",
    "CanvasRenderingContext2D.stroke", "CanvasRenderingContext2D.clip",
    "CanvasRenderingContext2D.isPointInPath", "CanvasRenderingContext2D.isPointInStroke",
    "CanvasRenderingContext2D.fillRect", "CanvasRenderingContext2D.strokeRect",
    "CanvasRenderingContext2D.clearRect", "CanvasRenderingContext2D.fillText",
    "CanvasRenderingContext2D.strokeText", "CanvasRenderingContext2D.measureText",
    "CanvasRenderingContext2D.drawImage",
    "CanvasRenderingContext2D.createLinearGradient",
    "CanvasRenderingContext2D.createRadialGradient",
    "CanvasRenderingContext2D.createConicGradient",
    "CanvasRenderingContext2D.createPattern", "CanvasRenderingContext2D.createImageData",
    "CanvasRenderingContext2D.getImageData", "CanvasRenderingContext2D.putImageData",
    "CanvasRenderingContext2D.shadowBlur", "CanvasRenderingContext2D.shadowColor",
    "CanvasRenderingContext2D.shadowOffsetX", "CanvasRenderingContext2D.shadowOffsetY",
    "CanvasRenderingContext2D.filter", "CanvasRenderingContext2D.letterSpacing",
    "CanvasRenderingContext2D.wordSpacing", "CanvasRenderingContext2D.fontKerning",
    "CanvasRenderingContext2D.getContextAttributes",
    "CanvasRenderingContext2D.drawFocusIfNeeded",
    "Path2D.moveTo", "Path2D.lineTo", "Path2D.bezierCurveTo", "Path2D.quadraticCurveTo",
    "Path2D.arc", "Path2D.arcTo", "Path2D.ellipse", "Path2D.rect", "Path2D.roundRect",
    "Path2D.closePath", "Path2D.addPath",
    "CanvasGradient.addColorStop", "CanvasPattern.setTransform"],
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
  WEB_GAMEPAD: ["Gamepad", "GamepadButton", "GamepadEvent", "GamepadHapticActuator",
    "Navigator.getGamepads", "GamepadHapticActuator.playEffect", "GamepadHapticActuator.reset"],
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
export const NATIVE = {
  app: ["dataDir", "cacheDir", "configDir", "requestSingleInstanceLock", "relaunch",
    "onQuitRequest", "onSuspend", "onResume", "registerProtocol", "registerFileAssociation"],
  window: ["setSize", "setFullscreen", "isFullscreen", "setDecorations", "isDecorated",
    "setMinimized", "setMaximized", "isMaximized", "startDrag", "close", "setAlwaysOnTop",
    "setCursor", "setCursorVisible", "setCursorGrab", "monitors",
    "create", "setTransparent", "isAlwaysOnTop", "startFileDrag"],
  dialog: ["openFile", "openFiles", "saveFile", "openFolder", "openFolders", "message"],
  clipboard: ["readText", "readHtml", "readImage", "writeText", "writeHtml", "writeImage",
    "clear", "readMime", "writeMime"],
  tray: ["configure", "remove", "onClick", "onAction"],
  menu: ["configure", "remove", "onAction"],
  input: ["snapshot", "gamepads", "vibrateGamepad", "onDeviceChange"],
  hid: ["devices", "open", "onDeviceChange"],
  notify: ["show", "permission", "requestPermission", "update", "close", "onEvent"],
  os: ["cpu", "memory", "storage", "host", "displays", "batteries", "locale", "idleTime"],
};

// Why a declared member is not implemented. Absence is the answer, not an
// oversight: the member is `undefined`, so `if (app.onQuitRequest)` selects a
// fallback, and this is what the documentation says about each one.
export const NATIVE_ABSENT = {
  "input.gamepads": "The standard navigator.getGamepads surface already carries every observable "
    + "controller field. A second native snapshot would only create a competing source of truth.",
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
  "window.create": "The architecture is decided: every future window gets an isolated Window, "
    + "Document, JavaScript heap and evaluated module graph, scheduled with the other windows on "
    + "one OS UI thread. Application data crosses an explicitly transferred MessagePort; no "
    + "context receives another window's global. The per-window host state and opaque lifecycle "
    + "capability needed to uphold that contract are not implemented, so this release exposes no "
    + "create member. The rest of this module operates on the calling document's native window.",
  "window.setTransparent": "Transparency is chosen when a window is created — winit's own setter "
    + "does nothing on X11 after that — so honouring it would mean replacing the window, which is "
    + "`create`. Run `blitsen` against a directory whose window should be transparent and the "
    + "attribute belongs on that window, not on a call.",
  "window.startFileDrag": "Dropping *into* the window is winit's to report and is implemented; "
    + "dragging *out* of it is not something winit can start. A drag source is a platform object "
    + "driven from the thread that owns the window — `IDropSource` with `DoDragDrop`, an "
    + "`NSDraggingSession`, a `wl_data_device` offer — and the first two run a modal loop that "
    + "does not return until the drop, on the one thread Blitsen keeps free to paint. That is a "
    + "design question rather than a missing call, so the module says so instead of answering it.",
  "window.isAlwaysOnTop": "winit sets the window level and cannot read it back, and the window "
    + "manager may change it without telling the application. Remembering what was last set would "
    + "be a second source of truth that quietly goes stale.",
  "clipboard.readMime": "`arboard` reads the flavours above and no others. Arbitrary MIME needs a "
    + "different mechanism on each platform — X11 selection targets, `wl_data_offer`, "
    + "`NSPasteboardType`, a registered Windows format — and no part of that is shared.",
  "clipboard.writeMime": "The counterpart of `readMime`, absent for the same reason.",
  "os.displays": "The monitors are `window.monitors()`, which already reports each one's size, "
    + "position and scale factor. A second list here could disagree with that one.",
  "os.idleTime": "Seconds since the last input is a different mechanism on every platform — the "
    + "X11 screensaver extension, `CGEventSourceSecondsSinceLastEventType`, `GetLastInputInfo` — "
    + "and Wayland has no answer at all for a client that is not focused: the idle-notify "
    + "protocol reports crossing a threshold the compositor was asked about, not a duration. "
    + "Reporting zero on the sessions that cannot answer would be indistinguishable from a "
    + "machine in use. It is also the one reading in this module that describes the person "
    + "rather than the machine — how long they have been away from the keyboard, available to "
    + "any application that asks for it — so implementing it where it happens to work would buy "
    + "that signal on three platforms in exchange for a wrong answer on the fourth.",
};

// Implemented native members that are deliberately omitted on named targets.
// The bootstrap keeps these keys in its source so the generated type surface
// stays optional, then `nativeMembers` removes their undefined runtime values.
export const NATIVE_CONDITIONAL = {
  "input.vibrateGamepad": {
    platforms: ["android"],
    reason: "The controller backend has no Android implementation, so there is no discovered "
      + "slot or actuator to address.",
  },
  "input.onDeviceChange": {
    platforms: ["android"],
    reason: "The controller backend has no Android implementation, so there are no controller "
      + "connection changes to report.",
  },
};

// Globals the *engine* supplies rather than the bridge, so their status cannot
// be read out of `dom_bridge.rs` like everything else here.
//
// Blitsen hosts QuickJS-ng (spikes/s8), which ships
// no `Intl` and no `WebAssembly`. Nothing in Blitsen deletes them — they were
// never installed — so the bootstrap-deletion invariants below do not apply and
// these are declared here instead. `cli-doctor.test.mjs` runs the built runtime
// and fails if it disagrees, which is what keeps this list from drifting into
// fiction the way an unverified declaration would.
export const ENGINE_ABSENT = {
  WEB_WASM: ["WebAssembly"],
};

// APIs the bootstrap installs and the host withdraws when the process it is
// running in cannot carry them. Implemented, and unconditional everywhere the
// entry does not name.
//
// This is the web counterpart of the target and run-time gates
// `native-modules.mjs` keeps. Some entries are a build fact (the controller
// backend is not compiled on Android); others are decided by the process (a
// macOS notification facade needs the identity of the process actually
// running). Both fail the same way for application code: the API is withdrawn.
//
// Declared rather than derived, for the reason `native-modules.mjs` gives about
// its own table: the gate is a `cfg` and a run-time call in `crates/blitsen-host`
// and a `cfg` is resolved by the compiler rather than visible in the source this
// reads. What *is* derived is the withdrawal — the bootstrap has to contain the
// deletion or the manifest refuses to build — so a condition can never be
// claimed for a global the runtime keeps regardless, and a global the runtime
// can drop can never go unexplained.
//
// This changes nothing about how an application is written: the API is absent
// when the condition does not hold, so `"Notification" in globalThis` selects a
// fallback exactly as it does for an absence that is a build fact.
export const CONDITIONAL = {
  Gamepad: {
    platforms: ["android"],
    reason: "The maintained controller backend supports Linux, macOS and Windows, but has no "
      + "Android backend. The API is absent there rather than returning an always-empty snapshot.",
  },
  GamepadButton: {
    platforms: ["android"],
    reason: "This snapshot type is installed with the desktop Gamepad API and absent with it.",
  },
  GamepadEvent: {
    platforms: ["android"],
    reason: "Controller connection events require the desktop controller backend.",
  },
  GamepadHapticActuator: {
    platforms: ["android"],
    reason: "Dual-rumble is exposed only where the desktop controller backend can address it.",
  },
  "Navigator.getGamepads": {
    platforms: ["android"],
    reason: "The maintained controller backend supports Linux, macOS and Windows, but has no "
      + "Android backend. The member is absent there rather than returning an always-empty array.",
  },
  Notification: {
    platforms: ["darwin", "android"],
    reason: "macOS notifications are `UNUserNotificationCenter`, which needs a bundle identity to "
      + "address and to hold permission against — and answers a process that has none by aborting "
      + "it rather than by failing the call, so the facade cannot be installed and left to throw. "
      + "An exported `.app` carries that identity and a development run of the interpreter does "
      + "not; `blitsen run --dev-bundle` gives the development host one of its own rather than "
      + "borrowing an installed application's. A process cannot acquire or lose a bundle "
      + "identifier while it runs, so the question is settled once, as the runtime installs "
      + "(#253). On Android the same question has a different subject: the facade's `click` is a "
      + "body tap, and a body tap there is a `PendingIntent` addressed to an installed "
      + "application identity, so a package the platform launched has one and a runtime started "
      + "against a directory standing in for `assets/` does not — a `Notification` whose "
      + "`onclick` could never fire would be a promise the constructor must not make (#252). "
      + "`blitsen/notify` is present either way and says why a call was refused.",
  },
};

// The `Intl` surface, declared rather than read out of the bootstrap.
//
// Everything else in this file is derived, and for a good reason — a claim and
// an implementation kept side by side drift. `Intl`'s formatters cannot be:
// they are classes inside a frozen object rather than globals, and the reader
// walks classes and globals. So they are declared here, with the owner each is
// reached through, and `runtime-surface.mjs` resolves every one of them against
// the real runtime and fails on a disagreement — the same guard ENGINE_ABSENT
// stands on, and the reason a declaration here is not a free claim.
export const DECLARED = {
  WEB_INTL: [
    ["Intl.NumberFormat", "Intl", "NumberFormat", true],
    ["Intl.DateTimeFormat", "Intl", "DateTimeFormat", true],
    ["Intl.RelativeTimeFormat", "Intl", "RelativeTimeFormat", true],
    ["Intl.PluralRules", "Intl", "PluralRules", true],
    ["Intl.Collator", "Intl", "Collator", true],
    ["Intl.ListFormat", "Intl", "ListFormat", true],
    ["Intl.getCanonicalLocales", "Intl", "getCanonicalLocales", true],
    ["Intl.NumberFormat.format", "Intl.NumberFormat.prototype", "format", true],
    ["Intl.NumberFormat.resolvedOptions", "Intl.NumberFormat.prototype", "resolvedOptions", true],
    ["Intl.DateTimeFormat.format", "Intl.DateTimeFormat.prototype", "format", true],
    ["Intl.DateTimeFormat.resolvedOptions", "Intl.DateTimeFormat.prototype", "resolvedOptions",
      true],
    ["Intl.Collator.compare", "Intl.Collator.prototype", "compare", true],
    ["Intl.PluralRules.select", "Intl.PluralRules.prototype", "select", true],
    ["Intl.ListFormat.format", "Intl.ListFormat.prototype", "format", true],
    ["Intl.RelativeTimeFormat.format", "Intl.RelativeTimeFormat.prototype", "format", true],
    // Absent, and each one for a reason COMPATIBILITY.md gives: the parts APIs
    // need pattern data ICU4X does not expose for every notation, and the three
    // formatters below are ICU4X components Blitsen does not link.
    ["Intl.NumberFormat.formatToParts", "Intl.NumberFormat.prototype", "formatToParts", false],
    ["Intl.DateTimeFormat.formatToParts", "Intl.DateTimeFormat.prototype", "formatToParts", false],
    ["Intl.DateTimeFormat.formatRange", "Intl.DateTimeFormat.prototype", "formatRange", false],
    ["Intl.Segmenter", "Intl", "Segmenter", false],
    ["Intl.DisplayNames", "Intl", "DisplayNames", false],
    ["Intl.DurationFormat", "Intl", "DurationFormat", false],
    ["Intl.supportedValuesOf", "Intl", "supportedValuesOf", false],
  ],
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
export const DIAGNOSTICS = {
  WEB_INTL: ["warning", "This part of Intl is not implemented; the formatters are.",
    "NumberFormat, DateTimeFormat — including named IANA time zones — RelativeTimeFormat, "
    + "PluralRules, Collator and ListFormat are implemented over CLDR, and so are the "
    + "`toLocale*` methods and `localeCompare` built on them. What is absent is the "
    + "`formatToParts`/`formatRange` family, Segmenter, DisplayNames and DurationFormat: "
    + "feature-detect them, or format the whole string and split it yourself."],
  WEB_WASM: ["warning", "WebAssembly is not implemented by the JavaScript engine Blitsen hosts.",
    "Ship a JavaScript build of the module, or keep the work in a native addon."],
  WEB_DOM: ["warning", "This DOM method is not implemented.",
    "Use the document-level lookups and node methods listed in COMPATIBILITY.md."],
  WEB_FORM_CONTROLS: ["warning", "This form-control API is not implemented.",
    "Validate and select in application code; handle the cancelable submit event rather than "
    + "submitting the form."],
  WEB_SCHEDULING: ["warning", "Idle-callback scheduling is not implemented.",
    "Schedule the work with requestAnimationFrame or a timer."],
  WEB_STORAGE: ["warning", "IndexedDB is not implemented.",
    "Use a Node filesystem/database package, or the Web Storage APIs for session state."],
  WEB_WORKER: ["warning", "Shared and service workers are not implemented; dedicated Worker is.",
    "Use a dedicated Worker, which this runtime runs on a thread of its own, and keep whatever "
    + "the workers were sharing in the document that started them."],
  WEB_MESSAGING: ["warning", "BroadcastChannel is not implemented; MessageChannel and Worker are.",
    "Use an explicitly handed-over MessagePort for a worker or any future isolated native-window "
    + "context; there is no implicit application-wide message bus."],
  WEB_URL: ["warning", "Object URLs are not implemented; URL and URLSearchParams are.",
    "Pass the Blob itself to whatever was going to fetch the URL, or build a data: URL."],
  WEB_XHR: ["warning", "XMLHttpRequest is not implemented.", "Use fetch with an absolute URL."],
  WEB_STREAM: ["warning", "Streaming bodies are not implemented; a response is buffered whole.",
    "Read the response with text(), json(), or arrayBuffer().", /\.body\s*\.\s*getReader\b/],
  WEB_FORM: ["warning", "Multipart form bodies and file objects are not implemented.",
    "Send a string, Blob, ArrayBuffer, or typed array body."],
  WEB_CANVAS: ["warning", "This canvas API is not implemented; the 2D context is.",
    "Draw through getContext(\"2d\"), and feature-detect anything that needs a second "
    + "rendering target or a blur.", /\.getContext\s*\(\s*["'](?:webgl2?|webgpu|bitmaprenderer)/],
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
  WEB_TRANSFER: ["warning",
    "This part of DataTransfer is not implemented; a dropped file is a path, not a File.",
    "Read `event.dataTransfer.paths` and open each absolute path with your filesystem library. "
    + "`types` still contains \"Files\", so the check that decides whether to accept a drop is "
    + "unchanged. `setDragImage` draws for a drag out of the window, which cannot be started."],
  WEB_SELECTION: ["warning",
    "This part of the range API is not implemented; the boundary, text and geometry reads are.",
    "Edit the tree with the node methods rather than through a range: a range here measures "
    + "and compares, and does not cut."],
};

// Diagnostics that are not an absence: an implemented API used in a way an
// exported application cannot honour.
export const USAGE_RULES = [
  // fetch reads the files the application shipped (issue #125), so a literal
  // that names one is not a finding at all — `doctor` resolves it against the
  // output and only reports what is not there. The capture group is what it
  // resolves; a URL assembled at runtime has none, and is undiagnosable here.
  //
  // `fetch(new URL("./blip.wav", import.meta.url))` is the idiomatic spelling
  // and its literal is one level in, so the optional prefix reaches it. What it
  // names is relative to the *module*, not to the document, which is why the
  // resolution in `doctor.mjs` tries the scanned file's own directory too.
  ["WEB_FETCH", "error",
    "\\bfetch\\s*\\(\\s*(?:new\\s+URL\\s*\\(\\s*)?[\"'`](?!https?:\\/\\/)([^\"'`]*)[\"'`]",
    "fetch names a path this application does not ship, and there is no server behind it.",
    "Ship the file in the output, or request an absolute http(s) URL."],
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
export const ASSET_RULES = [
  ["html", "ASSET_REMOTE_SCRIPT", "warning",
    REMOTE_HTML_SCRIPT_PATTERN,
    "A remote <script src> is not fetched; it is skipped and the rest of the page runs.",
    "Bundle the script into the output directory and reference its local path."],
  ["html", "ASSET_REMOTE", "warning",
    REMOTE_HTML_ASSET_PATTERN, ...REMOTE_ASSET],
  ["css", "ASSET_REMOTE", "warning", REMOTE_CSS_ASSET_PATTERN, ...REMOTE_ASSET],
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
export const RENDERER_RULES = [
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
  // Issue #127: an entrypoint that loads source rather than built output. The
  // runtime refuses it at the point of failure; this is the same refusal at
  // build time, where the fix costs one command instead of a blank window.
  ["html", "HTML_SOURCE_ENTRY", "error",
    "<script\\b[^>]*\\bsrc\\s*=\\s*[\"'][^\"']*\\.(?:ts|tsx|mts|cts|jsx|vue|svelte)(?:[?#][^\"']*)?[\"']",
    "This document loads source, not built output; nothing in Blitsen transpiles it.",
    "Run your bundler (Vite: `vite build`) and point Blitsen at its output directory."],
  ["html", "HTML_MEDIA", "warning", "<(?:video|track)\\b",
    "Video and text tracks are not implemented; <audio> is.",
    "Ship moving pictures as DOM, images and CSS, or feature-detect the media path."],
  // Issue #238: an `<svg>` element paints now — shapes, paths, `viewBox`,
  // `transform`, fills and strokes including `currentColor`, gradients, a
  // single-path `clipPath`, and text — so warning about every one of them
  // would be warning about the working case. What is left is this list, and it
  // is narrow on purpose: a `<pattern>` fill is not merely unpainted but marks
  // the frame's top-left corner with a half-transparent red box (gap G16), and
  // filters, masks and SMIL animation paint nothing.
  ["html", "HTML_SVG", "warning",
    "<(?:pattern|filter|mask|animate|animateTransform|animateMotion|set|foreignObject)\\b",
    "This SVG feature does not paint; shapes, paths, text, gradients and clipPath do.",
    "Flatten the effect into the shapes themselves, or rasterise this asset to PNG. A "
    + "`<pattern>` fill also leaves a red mark in the frame's corner, so it is worth removing "
    + "rather than tolerating."],
];

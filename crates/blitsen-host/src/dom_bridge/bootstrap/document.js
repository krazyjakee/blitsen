  const requireNode = value => {
    if (!(value instanceof Node) || !(handle in value)) throw new TypeError("argument is not a Node");
    return value[handle];
  };
  const wrapperCache = new Map();
  const TAG_INTERFACES = { "blitsen-view": BlitsenViewElement, button: HTMLButtonElement,
    canvas: HTMLCanvasElement,
    form: HTMLFormElement, img: HTMLImageElement, input: HTMLInputElement, link: HTMLLinkElement,
    option: HTMLOptionElement, select: HTMLSelectElement, style: HTMLStyleElement,
    template: HTMLTemplateElement, textarea: HTMLTextAreaElement };
  const wrap = rawHandle => {
    if (rawHandle == null) return null;
    rawHandle = String(rawHandle);
    const cached = wrapperCache.get(rawHandle);
    if (cached) return cached;
    const wrapper = __blitsenWrap(rawHandle);
    if (!(handle in wrapper)) {
      Object.defineProperty(wrapper, handle, { value: rawHandle });
      Object.setPrototypeOf(wrapper, call("kind", rawHandle) !== "element" ? Node.prototype
        : (TAG_INTERFACES[call("tagName", rawHandle)] ?? Element).prototype);
    }
    wrapperCache.set(rawHandle, wrapper);
    return wrapper;
  };

  class Document extends EventTarget {
    get pointerLockElement() { return pointerLockElement; }
    exitPointerLock() { exitPointerLock(); }
    get fullscreenElement() { return fullscreenElement; }
    get fullscreenEnabled() { return fullscreenSupported; }
    exitFullscreen() { return exitFullscreen(); }
    get nodeType() { return 9; }
    get nodeName() { return '#document'; }
    get ownerDocument() { return null; }
    querySelector(selector) { return wrap(call("querySelector", String(selector))); }
    querySelectorAll(selector) { return new NodeList(call("querySelectorAll", String(selector)).map(wrap)); }
    getElementsByTagName(name) { return this.querySelectorAll(String(name)); }
    getElementsByClassName(names) {
      return new NodeList(call("elementsByClassName", String(names)).map(wrap));
    }
    getElementById(id) { return wrap(call("getElementById", String(id))); }
    createElement(name) { return wrap(call("createElement", String(name))); }
    createElementNS(namespace, name) {
      return wrap(call("createElementNS", namespace == null ? "" : String(namespace), String(name)));
    }
    createTextNode(text) { return wrap(call("createTextNode", String(text))); }
    createComment(data) { return wrap(call("createComment", String(data))); }
    createDocumentFragment() { return createFragment(); }
    // There is one document, so importing a node is copying it.
    importNode(node, deep = false) { requireNode(node); return node.cloneNode(deep); }
    // Matched on the attribute rather than through a selector, for the reason
    // `getElementsByClassName` is: a name a framework generates is not
    // guaranteed to survive being spliced into selector syntax unescaped.
    getElementsByName(name) {
      name = String(name);
      return new NodeList([...this.querySelectorAll("[name]")]
        .filter(element => element.getAttribute("name") === name));
    }
    // Hit testing, which the native window already does for every click, asked
    // the other way round. `elementsFromPoint` is the same test read whole: the
    // path is what the hit walked through to reach the target.
    elementFromPoint(x, y) {
      return wrap(call("hitTest", Number(x), Number(y))?.target ?? null);
    }
    elementsFromPoint(x, y) {
      const hit = call("hitTest", Number(x), Number(y));
      return hit ? [wrap(hit.target), ...hit.path.map(wrap)] : [];
    }
    createRange() { return new Range(); }
    // The same laid-out text a range measures, asked the other way round: which
    // character is under this point. Answered as a collapsed range under the
    // name WebKit gave it and as a position under the name the standard gave
    // it — the two are the same reading, and a caller has one or the other
    // spelling compiled into it.
    caretRangeFromPoint(x, y) {
      const caret = this.caretPositionFromPoint(x, y);
      if (caret === null) return null;
      const range = this.createRange();
      range.setStart(caret.offsetNode, caret.offset);
      range.collapse(true);
      return range;
    }
    caretPositionFromPoint(x, y) {
      const caret = recordForcedLayout(call("caretPosition", Number(x), Number(y)));
      return caret.node === null ? null : new CaretPosition(wrap(caret.node), caret.offset);
    }
    getSelection() { return getSelection(); }
    get body() { return wrap(call("body")); }
    get head() { return this.querySelector("head"); }
    get documentElement() { return wrap(call("documentElement")); }
    // The element the document's own scroll offsets live on, and the one
    // `window.scrollTo` moves. Standards mode, so it is the root element.
    get scrollingElement() { return this.documentElement; }
    // The `<title>` element's text, created on demand when the document has
    // none: setting the title of a document without one is how a single-page
    // application gives itself a window title.
    get title() { return this.querySelector("title")?.textContent ?? ""; }
    set title(value) {
      let element = this.querySelector("title");
      if (!element) (this.head ?? this.documentElement).appendChild(element = this.createElement("title"));
      element.textContent = String(value);
    }
    get dir() { return this.documentElement?.getAttribute("dir") ?? ""; }
    set dir(value) { this.documentElement?.setAttribute("dir", value); }
    // Blitz decodes every document and subresource as UTF-8, so this is a fact
    // about the runtime rather than a value sniffed out of the document.
    get characterSet() { return "UTF-8"; }
    get documentURI() { return location.href; }
    // Whether this window is the focused one, which is not the same question as
    // whether anything inside it is focused. The native window reports its own
    // focus changes as lifecycle events, and this is that state read back.
    hasFocus() { return windowHasFocus(); }
    // There is one document, so adopting a node into it is a no-op that returns
    // the node — the same reasoning `importNode` is a clone under.
    adoptNode(node) { requireNode(node); return node; }
    get defaultView() { return globalThis; }
    // The same Location the window exposes. Assignment stays absent for the same
    // reason `location.href =` throws: it would be a navigation, which is not.
    get location() { return location; }
    get activeElement() { return activeElement?.isConnected ? activeElement : this.body; }
    get readyState() { return readyState; }
    createEvent(name) {
      const Interface = LEGACY_EVENT_INTERFACES[String(name).toLowerCase()];
      if (!Interface) {
        throw new DOMException(`document.createEvent does not support "${name}"`, "NotSupportedError");
      }
      return new Interface("");
    }
    // The sheets the cascade is actually reading, in the order it applies them.
    // A snapshot, like every collection here; the sheet objects in it are the
    // same ones `element.sheet` hands out.
    get styleSheets() { return new StyleSheetList(call("styleSheets").map(wrap).map(sheetFor)); }
  }

  // `Document` is a `Node` in the specification and an `EventTarget` here — the
  // document is the one node with no handle into the tree — so the node-type
  // constants every other node inherits have to be declared on it directly.
  defineNodeTypeConstants(Document);

  const document = new Document();
  class HTMLElement {
    static [Symbol.hasInstance](value) { return value instanceof Element; }
  }
  class HTMLIFrameElement {
    static [Symbol.hasInstance](value) {
      return value instanceof Element && elementTag(value) === "iframe";
    }
  }
  class Text {
    constructor(data = "") { return document.createTextNode(data); }
    static [Symbol.hasInstance](value) { return value instanceof Node && value.nodeType === 3; }
  }
  class Comment {
    constructor(data = "") { return document.createComment(data); }
    static [Symbol.hasInstance](value) { return value instanceof Node && value.nodeType === 8; }
  }
  class SVGElement {
    static [Symbol.hasInstance](value) {
      return value instanceof Element && value.namespaceURI === "http://www.w3.org/2000/svg";
    }
  }
  const imageDimension = value => {
    const number = Math.trunc(Number(value));
    return Number.isFinite(number) ? number : 0;
  };
  class Image {
    // The two arguments are the content attributes a browser writes, not a
    // layout size; an argument left out sets no attribute at all.
    constructor(width, height) {
      const image = document.createElement("img");
      if (width !== undefined) image.setAttribute("width", imageDimension(width));
      if (height !== undefined) image.setAttribute("height", imageDimension(height));
      return image;
    }
    static [Symbol.hasInstance](value) { return value instanceof HTMLImageElement; }
  }

  // What `parseFromString` hands back. There is one document in this runtime, so
  // a parsed string cannot become a second one: it becomes a detached fragment,
  // and `body` and `documentElement` are that fragment. The consequence is worth
  // being explicit about — the fragment parser drops `<html>`, `<head>` and
  // `<body>` tags, so a parsed document is not split into head and body, and
  // `head` is therefore null rather than an empty element that would read as a
  // document with nothing in its head. What this does serve is what callers
  // actually do with it: walk, query, and read text back out of `body`.
  class ParsedDocument extends DocumentFragment {
    get body() { return this; }
    get documentElement() { return this; }
    get head() { return null; }
    get title() { return this.querySelector("title")?.textContent ?? ""; }
    getElementById(id) { return this.querySelector(`[id="${String(id).replace(/["\\]/g, "\\$&")}"]`); }
    getElementsByTagName(name) { return this.querySelectorAll(String(name)); }
  }

  // Only `text/html`. The backend's parser is an HTML parser, and running XML
  // through it would silently mis-parse rather than fail — so an XML type is
  // refused, which a caller can feature-detect, instead of answered wrongly.
  class DOMParser {
    parseFromString(text, type = "text/html") {
      if (String(type) !== "text/html")
        throw new TypeError(`DOMParser supports text/html only, not "${type}"`);
      const parsed = Object.setPrototypeOf(createFragment(), ParsedDocument.prototype);
      // Through the operation rather than an `innerHTML` setter: the fragment's
      // interface is `Node`'s, and the backing node is the detached element a
      // fragment is parked under.
      call("setInnerHTML", parsed[handle], String(text));
      return parsed;
    }
  }

  // `CSS.escape` is what a library calls before putting a generated identifier
  // into a selector, and it is pure string work — the algorithm is CSSOM's, not
  // an approximation of it.
  const cssEscape = value => {
    const text = String(value);
    let escaped = "";
    for (let index = 0; index < text.length; index += 1) {
      const code = text.charCodeAt(index);
      if (code === 0) { escaped += "\uFFFD"; continue; }
      const character = text[index];
      if ((code >= 0x1 && code <= 0x1f) || code === 0x7f
        || (index === 0 && code >= 0x30 && code <= 0x39)
        || (index === 1 && code >= 0x30 && code <= 0x39 && text.charCodeAt(0) === 0x2d)) {
        escaped += `\\${code.toString(16)} `;
        continue;
      }
      if (index === 0 && code === 0x2d && text.length === 1) { escaped += `\\${character}`; continue; }
      if (code >= 0x80 || code === 0x2d || code === 0x5f
        || (code >= 0x30 && code <= 0x39) || (code >= 0x41 && code <= 0x5a)
        || (code >= 0x61 && code <= 0x7a)) {
        escaped += character;
        continue;
      }
      escaped += `\\${character}`;
    }
    return escaped;
  };
  // Answered by the cascade's own parser rather than by a table kept here: a
  // declaration that parses is one that round-trips through an inline style, and
  // one that does not leaves the property empty. That is how the property is
  // known to be supported *by this runtime*, which is the only useful answer.
  const cssSupports = (property, value) => {
    // The one-argument form takes a whole condition. Only the plain
    // `(property: value)` shape is understood; a condition with `and`, `or` or
    // `not` in it is not decomposed, and says so by answering false.
    if (value === undefined) {
      const condition = /^\s*\(\s*([\w-]+)\s*:\s*([^()]+?)\s*\)\s*$/.exec(String(property));
      if (!condition) return false;
      return cssSupports(condition[1], condition[2]);
    }
    const probe = document.createElement("div");
    probe.style.setProperty(String(property), String(value));
    return probe.style.getPropertyValue(String(property)) !== "";
  };
  const CSS = Object.freeze({ escape: cssEscape, supports: cssSupports });

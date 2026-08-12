  const requireNode = value => {
    if (!(value instanceof Node) || !(handle in value)) throw new TypeError("argument is not a Node");
    return value[handle];
  };
  const wrapperCache = new Map();
  const TAG_INTERFACES = { "blitsen-view": BlitsenViewElement, button: HTMLButtonElement,
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
    get body() { return wrap(call("body")); }
    get head() { return this.querySelector("head"); }
    get documentElement() { return wrap(call("documentElement")); }
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

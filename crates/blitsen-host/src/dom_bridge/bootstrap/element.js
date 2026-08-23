  // A resolved length as a number. Anything the cascade could not resolve to
  // one — `auto`, a keyword, the empty string — is 0, which is what every
  // caller of these means by "no border".
  const resolvedLength = (element, property) => {
    const value = Number.parseFloat(getComputedStyle(element).getPropertyValue(property));
    return Number.isFinite(value) ? value : 0;
  };
  const SCROLLING_OVERFLOWS = ["auto", "scroll", "overlay"];
  const isScrollable = element => {
    const style = getComputedStyle(element);
    return SCROLLING_OVERFLOWS.includes(style.getPropertyValue("overflow-y"))
      || SCROLLING_OVERFLOWS.includes(style.getPropertyValue("overflow-x"));
  };
  // How far a container must move along one axis to satisfy an alignment.
  // `nearest` is the one that can answer zero: a box already inside the
  // container does not move, and one outside moves by the smaller of the two
  // edge distances rather than to an edge it was not asked for.
  const scrollDelta = (alignment, start, end, containerStart, containerEnd) => {
    if (alignment === "start") return start - containerStart;
    if (alignment === "end") return end - containerEnd;
    if (alignment === "center") return (start + end) / 2 - (containerStart + containerEnd) / 2;
    if (start >= containerStart && end <= containerEnd) return 0;
    return Math.abs(start - containerStart) < Math.abs(end - containerEnd)
      ? start - containerStart : end - containerEnd;
  };
  // The displays that put their contents on their own line. Anything else —
  // `inline`, `inline-block`, `contents` — runs on with the text around it.
  const BLOCK_DISPLAY = /^(block|flex|grid|list-item|table|flow-root)/;
  // Appends one node's rendered text to `lines`, which the caller reads back as
  // the paragraphs `innerText` reports. A hidden subtree is skipped whole:
  // `display:none` is not laid out and `visibility:hidden` is laid out but not
  // painted, and neither is text a user could read.
  const appendRenderedText = (node, lines) => {
    if (node.nodeType === 3) {
      lines[lines.length - 1] += node.textContent.replace(/\s+/g, " ");
      return;
    }
    if (node.nodeType !== 1) return;
    const style = getComputedStyle(node);
    const display = style.getPropertyValue("display");
    if (display === "none" || style.getPropertyValue("visibility") === "hidden") return;
    if (elementTag(node) === "br") { lines.push(""); return; }
    const breaks = BLOCK_DISPLAY.test(display);
    if (breaks && lines[lines.length - 1] !== "") lines.push("");
    for (const child of node.childNodes) appendRenderedText(child, lines);
    if (breaks && lines[lines.length - 1] !== "") lines.push("");
  };

  class Element extends Node {
    requestPointerLock(options = {}) { return requestPointerLock(this, options); }
    requestFullscreen(options = {}) { return requestFullscreen(this, options); }
    get tagName() {
      const name = elementTag(this);
      // Only HTML folds case, which is why `linearGradient` survives here.
      return this.namespaceURI === HTML_NAMESPACE ? name.toUpperCase() : name;
    }
    get localName() { return elementTag(this); }
    get namespaceURI() { return call("namespaceUri", this[handle]); }
    querySelector(selector) { return wrap(call("querySelectorIn", this[handle], String(selector))); }
    querySelectorAll(selector) {
      return new NodeList(call("querySelectorAllIn", this[handle], String(selector)).map(wrap));
    }
    getElementsByTagName(name) { return this.querySelectorAll(String(name)); }
    // Static, as every collection this runtime returns is: a re-query sees the
    // mutation, the collection handed out before it does not.
    getElementsByClassName(names) {
      return new NodeList(call("elementsByClassNameIn", this[handle], String(names)).map(wrap));
    }
    matches(selector) { return call("matches", this[handle], String(selector)); }
    closest(selector) { return wrap(call("closest", this[handle], String(selector))); }
    get children() { return new NodeList(call("childElements", this[handle]).map(wrap)); }
    get childElementCount() { return call("childElements", this[handle]).length; }
    get firstElementChild() { return this.children[0] ?? null; }
    get lastElementChild() { const children = this.children; return children[children.length - 1] ?? null; }
    get nextElementSibling() { return wrap(call("nextElementSibling", this[handle])); }
    get previousElementSibling() { return wrap(call("previousElementSibling", this[handle])); }
    append(...nodes) { for (const node of nodes) this.appendChild(insertable(node)); }
    prepend(...nodes) {
      const reference = this.firstChild;
      for (const node of nodes) this.insertBefore(insertable(node), reference);
    }
    replaceChildren(...nodes) {
      for (const child of [...this.childNodes]) this.removeChild(child);
      this.append(...nodes);
    }
    get dataset() {
      let data = datasetCache.get(this);
      if (!data) { data = datasetMap(this); datasetCache.set(this, data); }
      return data;
    }
    getAttribute(name) { return call("getAttribute", this[handle], String(name)); }
    setAttribute(name, value) {
      name = String(name);
      const oldValue = this.getAttribute(name);
      call("setAttribute", this[handle], name, String(value));
      notifyMutation({ type: "attributes", target: this, attributeName: name,
        attributeNamespace: null, oldValue });
    }
    removeAttribute(name) {
      name = String(name);
      const oldValue = this.getAttribute(name);
      call("removeAttribute", this[handle], name);
      notifyMutation({ type: "attributes", target: this, attributeName: name,
        attributeNamespace: null, oldValue });
    }
    hasAttribute(name) { return call("hasAttribute", this[handle], String(name)); }
    hasAttributes() { return this.getAttributeNames().length > 0; }
    getAttributeNames() { return call("attributeNames", this[handle]); }
    get attributes() { return new NamedNodeMap(this); }
    toggleAttribute(name, force) {
      name = String(name);
      const present = this.hasAttribute(name);
      const wanted = force === undefined ? !present : Boolean(force);
      if (wanted !== present) {
        if (wanted) this.setAttribute(name, ""); else this.removeAttribute(name);
      }
      return wanted;
    }
    // The namespaced half of the attribute surface, which is how React and Vue
    // write `xlink:href` and `xml:space`. A namespace of null is the space the
    // plain accessors above use, so the two halves reach the same attribute.
    getAttributeNS(namespace, name) {
      return call("getAttributeNS", this[handle], namespace == null ? "" : String(namespace), String(name));
    }
    setAttributeNS(namespace, name, value) {
      namespace = namespace == null ? "" : String(namespace);
      name = String(name);
      const oldValue = this.getAttributeNS(namespace, name);
      call("setAttributeNS", this[handle], namespace, name, String(value));
      notifyMutation({ type: "attributes", target: this, attributeName: name,
        attributeNamespace: namespace || null, oldValue });
    }
    removeAttributeNS(namespace, name) {
      namespace = namespace == null ? "" : String(namespace);
      name = String(name);
      const oldValue = this.getAttributeNS(namespace, name);
      call("removeAttributeNS", this[handle], namespace, name);
      notifyMutation({ type: "attributes", target: this, attributeName: name,
        attributeNamespace: namespace || null, oldValue });
    }
    get id() { return this.getAttribute("id") ?? ""; }
    set id(value) { this.setAttribute("id", value); }
    get className() { return this.getAttribute("class") ?? ""; }
    set className(value) { this.setAttribute("class", value); }
    get classList() {
      let list = classListCache.get(this);
      if (!list) {
        list = new DOMTokenList(this, "class");
        classListCache.set(this, list);
      }
      return list;
    }
    get style() {
      let style = styleCache.get(this);
      if (!style) {
        const declaration = new CSSStyleDeclaration(this);
        style = new Proxy(declaration, {
          get(target, property, receiver) {
            if (typeof property !== "string" || property in target) return Reflect.get(target, property, receiver);
            return target._getJsProperty(property);
          },
          set(target, property, value, receiver) {
            if (typeof property !== "string" || property in target) return Reflect.set(target, property, value, receiver);
            target._setJsProperty(property, String(value));
            return true;
          }
        });
        styleCache.set(this, style);
      }
      return style;
    }
    get innerHTML() { return call("innerHTML", this[handle]); }
    set innerHTML(value) { call("setInnerHTML", this[handle], String(value)); }
    get outerHTML() { return call("outerHTML", this[handle]); }
    insertAdjacentHTML(position, html) {
      position = String(position);
      const inserted = call("insertAdjacentHTML", this[handle], position, String(html)).map(wrap);
      if (inserted.length === 0) return;
      const target = /^(?:beforebegin|afterend)$/i.test(position) ? this.parentNode : this;
      notifyMutation({ type: "childList", target, addedNodes: new NodeList(inserted),
        removedNodes: new NodeList([]), previousSibling: inserted[0].previousSibling,
        nextSibling: inserted[inserted.length - 1].nextSibling });
    }
    insertAdjacentElement(position, element) {
      if (!(element instanceof Element)) throw new TypeError("argument is not an Element");
      const parent = this.parentNode;
      switch (String(position).toLowerCase()) {
        // A node with no parent has no "before" or "after" to insert into, and
        // the DOM says that is a null return rather than an error.
        case "beforebegin": return parent ? (parent.insertBefore(element, this), element) : null;
        case "afterbegin": return this.insertBefore(element, this.firstChild), element;
        case "beforeend": return this.appendChild(element), element;
        case "afterend": return parent ? (parent.insertBefore(element, this.nextSibling), element) : null;
        default: throw new DOMException(
          `insertAdjacentElement does not support "${position}"`, "SyntaxError");
      }
    }
    // Rendered text, which is the whole of what separates it from
    // `textContent`: a subtree the cascade has hidden contributes nothing, and a
    // block boundary is a line break. What it does not do is re-derive line
    // wrapping — this reads the tree and its computed display, not Blitz's line
    // boxes, so a paragraph that wrapped over three lines is still one line
    // here. Writing it is the inverse: newlines become `<br>`.
    get innerText() {
      const lines = [""];
      for (const child of this.childNodes) appendRenderedText(child, lines);
      while (lines.length && lines[0].trim() === "") lines.shift();
      while (lines.length && lines[lines.length - 1].trim() === "") lines.pop();
      return lines.map(line => line.trim()).join("\n");
    }
    set innerText(value) {
      this.replaceChildren();
      String(value).split("\n").forEach((part, index) => {
        if (index > 0) this.appendChild(document.createElement("br"));
        if (part) this.appendChild(document.createTextNode(part));
      });
    }
    // Reflected content attributes. `hidden` is the boolean form — present is
    // true whatever the value reads — and `title` is the plain string one.
    get hidden() { return this.hasAttribute("hidden"); }
    set hidden(value) { this.toggleAttribute("hidden", Boolean(value)); }
    get title() { return this.getAttribute("title") ?? ""; }
    set title(value) { this.setAttribute("title", value); }
    // A missing `tabindex` is 0 on something focusable and -1 on everything
    // else. That is the default the DOM defines, and `isFocusable` is the same
    // predicate the runtime's own focus walk uses, so the two cannot disagree.
    get tabIndex() {
      const declared = Number.parseInt(this.getAttribute("tabindex"), 10);
      return Number.isInteger(declared) ? declared : (isFocusable(this) ? 0 : -1);
    }
    set tabIndex(value) {
      const number = Number.parseInt(value, 10);
      this.setAttribute("tabindex", String(Number.isInteger(number) ? number : 0));
    }
    // The border edge to the padding edge — the offset `clientWidth` and
    // `clientHeight` measure in from. Read off the resolved border widths
    // rather than differenced out of the two boxes, which would fold the
    // padding in as well.
    get clientTop() { return resolvedLength(this, "border-top-width"); }
    get clientLeft() { return resolvedLength(this, "border-left-width"); }
    // The box `offsetTop` and `offsetLeft` would be relative to: the nearest
    // positioned ancestor, or the body once the walk runs out. An element the
    // cascade is not laying out has none at all, which is how a library tells
    // "not rendered" from "rendered at the origin".
    get offsetParent() {
      if (this === document.body || this === document.documentElement) return null;
      if (getComputedStyle(this).getPropertyValue("display") === "none") return null;
      for (let parent = this.parentElement; parent; parent = parent.parentElement) {
        if (parent === document.body) return parent;
        if (getComputedStyle(parent).getPropertyValue("position") !== "static") return parent;
      }
      return null;
    }
    // Scrolls each scrolling ancestor, and then the document, until this box is
    // inside it. Measured as a delta between the two rectangles rather than
    // computed from scroll extents, because the backend clamps a scroll offset
    // it cannot honour and a delta therefore needs no extent to be correct.
    // `behavior` is accepted and ignored: there is no animation to run, so the
    // scroll simply lands.
    scrollIntoView(options = true) {
      const block = typeof options === "object" && options !== null
        ? String(options.block ?? "start") : (options === false ? "end" : "start");
      const inline = typeof options === "object" && options !== null
        ? String(options.inline ?? "nearest") : "nearest";
      for (let parent = this.parentElement; parent; parent = parent.parentElement) {
        if (parent !== document.documentElement && !isScrollable(parent)) continue;
        const box = this.getBoundingClientRect();
        const container = parent.getBoundingClientRect();
        parent.scrollTop += scrollDelta(block, box.top, box.bottom, container.top, container.bottom);
        parent.scrollLeft += scrollDelta(inline, box.left, box.right, container.left, container.right);
      }
    }
    // Pointer capture: every event from this pointer is retargeted here until
    // the contact ends or the capture is released, so a drag that leaves the
    // handle keeps arriving at the handle. The state and the retargeting live
    // beside the dispatcher in the events fragment; these are the DOM's names
    // for them. `hasPointerCapture` answers about the *requested* capture, which
    // is what a caller that has just set one expects to read back.
    setPointerCapture(pointerId) { pointerCaptureSet(this, pointerId); }
    releasePointerCapture(pointerId) { pointerCaptureRelease(this, pointerId); }
    hasPointerCapture(pointerId) { return pointerCaptureHas(this, pointerId); }
    // One box per line box the element was broken across, which for anything
    // with a box of its own is the single border box `getBoundingClientRect`
    // returns. An inline element is not laid out as a box at all — it is a run
    // of styled text inside its block — so one that wraps has a fragment per
    // line, and the bounding rectangle is only their union.
    getClientRects() {
      const { rects } = recordForcedLayout(call("clientRects", this[handle]));
      return Object.freeze(rects.map(rect => clientRect(rect.x, rect.y, rect.width, rect.height)));
    }
    getBoundingClientRect() {
      const { x, y, width, height } = recordForcedLayout(call("layoutMetrics", this[handle]));
      return clientRect(x, y, width, height);
    }
    get offsetWidth() { return recordForcedLayout(call("layoutMetrics", this[handle])).offsetWidth; }
    get offsetHeight() { return recordForcedLayout(call("layoutMetrics", this[handle])).offsetHeight; }
    get clientWidth() { return recordForcedLayout(call("layoutMetrics", this[handle])).clientWidth; }
    get clientHeight() { return recordForcedLayout(call("layoutMetrics", this[handle])).clientHeight; }
    get scrollLeft() { return recordForcedLayout(call("layoutMetrics", this[handle])).scrollLeft; }
    set scrollLeft(value) {
      const number = Number(value);
      recordForcedLayout(call("setScroll", this[handle], "left", String(Number.isNaN(number) ? 0 : number)));
    }
    get scrollTop() { return recordForcedLayout(call("layoutMetrics", this[handle])).scrollTop; }
    set scrollTop(value) {
      const number = Number(value);
      recordForcedLayout(call("setScroll", this[handle], "top", String(Number.isNaN(number) ? 0 : number)));
    }
    focus() { if (isFocusable(this)) setFocus(this); }
    blur() { if (activeElement === this) setFocus(document.body); }
  }

  // A fragment is backed by a detached element rather than by a list of nodes:
  // that gives its children a real parent to be parsed, serialized and cloned
  // against, and it is never connected, so it is never styled or painted.
  class DocumentFragment extends Node {
    get nodeType() { return 11; }
    get nodeName() { return "#document-fragment"; }
    cloneNode(deep = false) { return asFragment(super.cloneNode(deep)); }
    querySelector(selector) { return wrap(call("querySelectorIn", this[handle], String(selector))); }
    querySelectorAll(selector) {
      return new NodeList(call("querySelectorAllIn", this[handle], String(selector)).map(wrap));
    }
  }

  // Inserting a fragment moves its children and leaves it empty, which is the
  // whole of what a fragment is for.
  const insertFragment = (parent, fragment, reference) => {
    const moved = [...fragment.childNodes];
    const anchor = reference == null ? "" : requireNode(reference);
    for (const child of moved) call("insertBefore", parent[handle], child[handle], anchor);
    if (moved.length > 0) notifyMutation({ type: "childList", target: parent,
      addedNodes: new NodeList(moved), removedNodes: new NodeList([]),
      previousSibling: moved[0].previousSibling, nextSibling: reference });
    return fragment;
  };

  const templateContents = new WeakMap();

  // A fragment host is a template element the wrapper is retyped over: the
  // parser needs the element, and JavaScript needs the fragment interface.
  const asFragment = node => Object.setPrototypeOf(node, DocumentFragment.prototype);
  const createFragment = () => asFragment(wrap(call("createFragment")));

  class HTMLTemplateElement extends Element {
    // Blitz has no separate template-contents document, so a parsed template
    // keeps its children until they are asked for. Moving them into the
    // fragment on read is what makes `content` behave as the parser should
    // have: the element ends up empty and the nodes end up in the fragment.
    get content() {
      let fragment = templateContents.get(this);
      if (!fragment) templateContents.set(this, fragment = createFragment());
      for (const child of this.childNodes) fragment.appendChild(child);
      return fragment;
    }
  }

  // The `rel` keywords this runtime understands. `supports` is what Vite's
  // module-preload polyfill asks before installing itself, and answering
  // truthfully keeps it from fetching every chunk over an address with no
  // server behind it. The preload hints are honoured by doing nothing: an
  // exported application's chunks are local files that need no warming.
  const LINK_RELATIONS = ["alternate", "author", "canonical", "dns-prefetch", "help", "icon",
    "license", "manifest", "modulepreload", "next", "pingback", "preconnect", "prefetch",
    "preload", "prev", "search", "stylesheet"];

  // `load` and `error` for a subresource, shared by the elements that own one.
  // Blitz fetches these beside the DOM and announces nothing when one lands, so
  // both events are delivered by polling the elements that owe an outcome — at
  // the frame boundary, where `fetch` completions land too.
  //
  // Over a copy: a handler that gives another element a source owes that
  // outcome to the next frame, not to the rest of this pass. A `read` that
  // answers null means the element is no longer waiting on anything, so it
  // leaves the set without an event: there is no request to report the end of.
  const settleResources = (pending, read) => {
    for (const element of [...pending]) {
      const state = read(element);
      if (state && !state.complete) continue;
      pending.delete(element);
      if (state) element.dispatchEvent(new Event(state.errored ? "error" : "load"));
    }
  };
  // Blitz requests a subresource only once its element is in the document, so a
  // detached one is waiting on nothing and must not hold the host open.
  const waitingFor = pending => {
    let waiting = 0;
    for (const element of pending) if (element.isConnected) waiting++;
    return waiting;
  };
  // An `onload` property replaces the listener it installed, which is the one
  // thing a plain listener list cannot express — hence a handler held per
  // element beside it.
  const setResourceHandler = (registry, element, type, callback) => {
    let handlers = registry.get(element);
    if (!handlers) registry.set(element, handlers = { load: null, error: null });
    setEventHandler(element, handlers, type, callback);
  };

  // Linked stylesheets. Only `rel="stylesheet"` is ever waited on, because it is
  // the only `rel` Blitz requests anything for — a preload hint that pended here
  // would be waiting on a request nobody made, and holding the host open to do
  // it. That is what the renderer's `pending` answers, and why a link that stops
  // naming a stylesheet leaves the set silently rather than reporting an error.
  const pendingLinks = new Set();
  const linkHandlers = new WeakMap();
  const linkState = element => {
    const state = call("linkState", element[handle]);
    return state.pending ? state : null;
  };
  const setLinkHandler = (element, type, callback) =>
    setResourceHandler(linkHandlers, element, type, callback);
  const settleLinks = () => settleResources(pendingLinks, linkState);
  const waitingLinks = () => waitingFor(pendingLinks);

  class HTMLLinkElement extends Element {
    get relList() {
      let list = relListCache.get(this);
      if (!list) {
        list = new DOMTokenList(this, "rel", LINK_RELATIONS);
        relListCache.set(this, list);
      }
      return list;
    }
    get rel() { return this.getAttribute("rel") ?? ""; }
    set rel(value) { this.setAttribute("rel", value); }
    get href() {
      const value = this.getAttribute("href");
      return value === null ? "" : resolveAgainstDocument(value).href;
    }
    set href(value) { this.setAttribute("href", value); }
    // A linked sheet is in the cascade, so it is one of the document's sheets
    // and says so; what it cannot answer is `cssRules`, which is a file this
    // process fetched rather than text in the tree.
    get sheet() {
      return this.isConnected && this.relList.contains("stylesheet") ? sheetFor(this) : null;
    }
    // An `href` is a request whatever it resolves to, so the outcome is owed
    // from the write — which is also what makes a theme swap fire a second
    // time. Through `setAttribute` rather than the `href` setter because that
    // is the one a framework renders through.
    setAttribute(name, value) {
      super.setAttribute(name, value);
      if (String(name) === "href" && this.relList.contains("stylesheet")) pendingLinks.add(this);
    }
    // Nothing is delivered retroactively: a sheet that has already landed is
    // read through `document.styleSheets`, or simply through the styles it
    // resolved to.
    addEventListener(type, callback, options = false) {
      super.addEventListener(type, callback, options);
      if (type === "load" || type === "error") {
        const state = call("linkState", this[handle]);
        if (state.pending && !state.complete) pendingLinks.add(this);
      }
    }
    get onload() { return linkHandlers.get(this)?.load ?? null; }
    set onload(callback) { setLinkHandler(this, "load", callback); }
    get onerror() { return linkHandlers.get(this)?.error ?? null; }
    set onerror(callback) { setLinkHandler(this, "error", callback); }
  }

  // A `<style>` element's text is its sheet's source, which is what makes the
  // sheet writable: see the CSSOM objects below. A disconnected element has no
  // sheet because nothing it says has reached the cascade yet.
  class HTMLStyleElement extends Element {
    get sheet() { return this.isConnected ? sheetFor(this) : null; }
    get type() { return this.getAttribute("type") ?? ""; }
    set type(value) { this.setAttribute("type", value); }
  }

  // Images. Blitz decodes subresources beside the DOM and announces nothing when
  // one lands, so `load` and `error` are delivered by polling the elements that
  // owe an outcome — at the frame boundary, where `fetch` completions land too.
  const pendingImages = new Set();
  const imageHandlers = new WeakMap();
  const imageState = element => call("imageState", element[handle]);
  const setImageHandler = (element, type, callback) =>
    setResourceHandler(imageHandlers, element, type, callback);
  const settleImages = () => settleResources(pendingImages, imageState);
  const waitingImages = () => waitingFor(pendingImages);

  class HTMLImageElement extends Element {
    // Decoded size is applied while layout resolves, so reading it is a layout
    // read exactly as `getBoundingClientRect` is.
    get naturalWidth() { return recordForcedLayout(imageState(this)).naturalWidth; }
    get naturalHeight() { return recordForcedLayout(imageState(this)).naturalHeight; }
    get complete() { return recordForcedLayout(imageState(this)).complete; }
    get src() {
      const value = this.getAttribute("src");
      return value === null ? "" : resolveAgainstDocument(value).href;
    }
    set src(value) { this.setAttribute("src", value); }
    // A source is a request whatever it resolves to, so the outcome is owed from
    // the write. Through `setAttribute` rather than the `src` setter because
    // that is the one a framework renders through.
    setAttribute(name, value) {
      super.setAttribute(name, value);
      if (String(name) === "src") pendingImages.add(this);
    }
    // Nothing is delivered retroactively: an image that has already settled is
    // read through `complete`, which is what `complete` is for.
    addEventListener(type, callback, options = false) {
      super.addEventListener(type, callback, options);
      if ((type === "load" || type === "error") && !imageState(this).complete)
        pendingImages.add(this);
    }
    get onload() { return imageHandlers.get(this)?.load ?? null; }
    set onload(callback) { setImageHandler(this, "load", callback); }
    get onerror() { return imageHandlers.get(this)?.error ?? null; }
    set onerror(callback) { setImageHandler(this, "error", callback); }
  }

  // Acquired surfaces are held strongly: the element is what the application
  // draws into, and it releases the claim by releasing the surface.
  const acquiredSurfaces = new Map();
  const surfaceElements = new WeakMap();
  const surfaceElement = surface => {
    const element = surfaceElements.get(surface);
    if (!element) throw new TypeError("Illegal invocation");
    if (!acquiredSurfaces.has(element)) throw new DOMException("The surface has been released", "InvalidStateError");
    return element;
  };
  const surfaceInfo = surface =>
    recordForcedLayout(call("viewportSurface", surfaceElement(surface)[handle]));
  const notifySurfaceResizes = () => {
    for (const [element, record] of acquiredSurfaces) {
      const generation = call("viewportSurface", element[handle]).generation;
      if (generation === record.generation) continue;
      record.generation = generation;
      element.dispatchEvent(new Event("resize"));
    }
  };

  class BlitsenViewSurface {
    constructor(element) { surfaceElements.set(this, element); }
    // Physical-pixel dimensions: what the application must fill, not the CSS box.
    get width() { return surfaceInfo(this).width; }
    get height() { return surfaceInfo(this).height; }
    get devicePixelRatio() { return surfaceInfo(this).devicePixelRatio; }
    get generation() { return surfaceInfo(this).generation; }
    get byteLength() { return surfaceInfo(this).byteLength; }
    write(pixels) {
      const element = surfaceElement(this);
      if (!ArrayBuffer.isView(pixels)) throw new TypeError("surface contents must be a typed array");
      __blitsenViewportWrite(String(element[handle]), pixels);
    }
    release() { acquiredSurfaces.delete(surfaceElements.get(this)); }
  }

  class BlitsenViewElement extends Element {
    acquireSurface() {
      if (acquiredSurfaces.has(this))
        throw new DOMException("The surface is already acquired", "InvalidStateError");
      const generation = call("viewportSurface", this[handle]).generation;
      const surface = new BlitsenViewSurface(this);
      acquiredSurfaces.set(this, { surface, generation });
      return surface;
    }
  }

  class NodeList {
    constructor(items) {
      defineIndexed(this, items);
    }
    item(index) { return this[index] ?? null; }
    // The iteration members a `NodeList` is expected to have. `forEach` is the
    // one a bundle reaches for by name —
    // `document.querySelectorAll("style[data-id]").forEach(…)` is how Vite's own
    // client collects its style elements — and a list that is iterable but has
    // no `forEach` fails at exactly that line rather than at feature detection.
    forEach(callback, thisArg) {
      for (let index = 0; index < this.length; index++) {
        callback.call(thisArg, this[index], index, this);
      }
    }
    *entries() { for (let index = 0; index < this.length; index++) yield [index, this[index]]; }
    *keys() { for (let index = 0; index < this.length; index++) yield index; }
    *values() { for (let index = 0; index < this.length; index++) yield this[index]; }
    *[Symbol.iterator]() { for (let index = 0; index < this.length; index++) yield this[index]; }
  }

  // An attribute as an object. Only its name is captured: the value is read and
  // written through the element, so an attribute node that outlives a mutation
  // does not answer with the value it was made from. No prefix is stored — the
  // bridge keys an attribute by namespace and local name — so the qualified name
  // and the local one are the same string.
  class Attr {
    constructor(element, namespace, name) {
      this._element = element;
      this._namespace = namespace;
      this._name = name;
    }
    get ownerElement() { return this._element; }
    get namespaceURI() { return this._namespace; }
    get name() { return this._name; }
    get localName() { return this._name; }
    get value() { return this._element.getAttributeNS(this._namespace, this._name) ?? ""; }
    set value(value) { this._element.setAttributeNS(this._namespace, this._name, value); }
  }

  // Static, as every collection this runtime hands out is. The attribute nodes
  // in it are not: each of those still reads through the element.
  class NamedNodeMap {
    constructor(element) {
      const nodes = call("attributeEntries", element[handle])
        .map(entry => new Attr(element, entry.namespace, entry.name));
      defineIndexed(this, nodes);
    }
    item(index) { return this[index] ?? null; }
    getNamedItem(name) { return this.getNamedItemNS(null, name); }
    getNamedItemNS(namespace, name) {
      const uri = namespace == null ? null : String(namespace);
      name = uri === null ? String(name).toLowerCase() : String(name);
      for (const attribute of this) if (attribute.namespaceURI === uri && attribute.name === name) return attribute;
      return null;
    }
    *[Symbol.iterator]() { for (let index = 0; index < this.length; index++) yield this[index]; }
  }

  class DOMTokenList {
    constructor(element, attribute, supported = null) {
      this._element = element;
      this._attribute = attribute;
      this._supported = supported;
    }
    _text() { return (this._element.getAttribute(this._attribute) ?? "").trim(); }
    _tokens() { return this._text() ? this._text().split(/\s+/) : []; }
    _validate(tokens) {
      for (const token of tokens) {
        if (!token || /\s/.test(token)) throw new DOMException("The token must not be empty or contain whitespace", "SyntaxError");
      }
    }
    get length() { return this._tokens().length; }
    item(index) { return this._tokens()[index] ?? null; }
    contains(token) { this._validate([token]); return this._tokens().includes(token); }
    forEach(callback, thisArg) { this._tokens().forEach((token, index) => callback.call(thisArg, token, index, this)); }
    // Only a list with a defined keyword set answers this; the class attribute
    // has none, and the DOM says that is a TypeError rather than a false.
    supports(token) {
      if (!this._supported) throw new TypeError(`${this._attribute} has no supported tokens`);
      return this._supported.includes(String(token).toLowerCase());
    }
    add(...tokens) {
      this._validate(tokens);
      const values = this._tokens();
      for (const token of tokens) if (!values.includes(token)) values.push(token);
      this._element.setAttribute(this._attribute, values.join(" "));
    }
    remove(...tokens) {
      this._validate(tokens);
      this._element.setAttribute(this._attribute,
        this._tokens().filter(token => !tokens.includes(token)).join(" "));
    }
    toggle(token, force) {
      this._validate([token]);
      const present = this.contains(token);
      const desired = force === undefined ? !present : Boolean(force);
      if (desired !== present) (desired ? this.add(token) : this.remove(token));
      return desired;
    }
    toString() { return this._element.getAttribute(this._attribute) ?? ""; }
    *[Symbol.iterator]() { yield* this._tokens(); }
  }

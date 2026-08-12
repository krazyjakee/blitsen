  const NODE_TYPES = { element: 1, text: 3, comment: 8, document: 9, fragment: 11 };

  class Node extends EventTarget {
    constructor() { throw new TypeError("Illegal constructor"); }
    get nodeType() { return NODE_TYPES[call("kind", this[handle])]; }
    get nodeName() {
      const type = this.nodeType;
      return type === 1 ? this.tagName : type === 8 ? "#comment" : "#text";
    }
    get nodeValue() { return this.nodeType === 1 ? null : this.textContent; }
    set nodeValue(value) { this.textContent = value === null ? "" : value; }
    get ownerDocument() { return document; }
    appendChild(child) {
      if (child instanceof DocumentFragment) return insertFragment(this, child, null);
      call("appendChild", this[handle], requireNode(child));
      notifyMutation({ type: "childList", target: this, addedNodes: new NodeList([child]),
        removedNodes: new NodeList([]), previousSibling: child.previousSibling, nextSibling: null });
      return child;
    }
    insertBefore(child, reference) {
      if (child instanceof DocumentFragment) return insertFragment(this, child, reference ?? null);
      call("insertBefore", this[handle], requireNode(child), reference == null ? "" : requireNode(reference));
      notifyMutation({ type: "childList", target: this, addedNodes: new NodeList([child]),
        removedNodes: new NodeList([]), previousSibling: child.previousSibling, nextSibling: reference });
      return child;
    }
    before(...nodes) {
      const parent = this.parentNode;
      if (parent) for (const node of nodes) parent.insertBefore(node, this);
    }
    after(...nodes) {
      const parent = this.parentNode;
      if (parent) for (const node of nodes.reverse()) parent.insertBefore(node, this.nextSibling);
    }
    // A clone carries the tree and nothing else: no listeners and no wrapper
    // identity, which is what the DOM specifies.
    cloneNode(deep = false) { return wrap(call("cloneNode", this[handle], Boolean(deep))); }
    contains(other) {
      return other instanceof Node && call("contains", this[handle], other[handle]);
    }
    removeChild(child) {
      const previousSibling = child.previousSibling;
      const nextSibling = child.nextSibling;
      call("removeChild", this[handle], requireNode(child));
      notifyMutation({ type: "childList", target: this, addedNodes: new NodeList([]),
        removedNodes: new NodeList([child]), previousSibling, nextSibling });
      return child;
    }
    remove() { call("remove", this[handle]); }
    replaceWith(replacement) { call("replaceWith", this[handle], requireNode(replacement)); }
    get parentNode() { return wrap(call("parentNode", this[handle])); }
    get parentElement() {
      const parent = this.parentNode;
      return parent?.nodeType === 1 ? parent : null;
    }
    get childNodes() { return new NodeList(call("childNodes", this[handle]).map(wrap)); }
    get firstChild() { return wrap(call("firstChild", this[handle])); }
    get lastChild() { return wrap(call("lastChild", this[handle])); }
    get nextSibling() { return wrap(call("nextSibling", this[handle])); }
    get previousSibling() { return wrap(call("previousSibling", this[handle])); }
    get isConnected() { return call("isConnected", this[handle]); }
    // There are no shadow roots in this runtime, so a connected node's root is
    // the document itself rather than the element the parent walk stops at.
    getRootNode() {
      if (this.isConnected) return document;
      let root = this;
      for (let parent = root.parentNode; parent; parent = parent.parentNode) root = parent;
      return root;
    }
    // Merges adjacent text and drops the empty ones, depth first. A comment
    // between two text nodes separates them, which is why any other child ends
    // the run rather than being skipped over.
    normalize() {
      let run = null;
      for (const child of [...this.childNodes]) {
        if (child.nodeType !== 3) { run = null; child.normalize(); continue; }
        if (child.textContent === "") { child.remove(); continue; }
        if (!run) { run = child; continue; }
        run.textContent += child.textContent;
        child.remove();
      }
    }
    get textContent() { return call("textContent", this[handle]); }
    set textContent(value) {
      call("setTextContent", this[handle], String(value));
      notifyMutation({ type: "characterData", target: this, oldValue: null });
    }
  }

  const styleCache = new WeakMap();
  const classListCache = new WeakMap();
  const relListCache = new WeakMap();
  const datasetCache = new WeakMap();
  const HTML_NAMESPACE = "http://www.w3.org/1999/xhtml";
  // `data-my-value` is `dataset.myValue`, the DOMStringMap mapping both ways.
  const datasetName = key => `data-${String(key).replace(/[A-Z]/g, letter => `-${letter.toLowerCase()}`)}`;
  const datasetKey = name => name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
  const datasetMap = element => new Proxy({}, {
    get(_, key) {
      return typeof key === "string" ? element.getAttribute(datasetName(key)) ?? undefined : undefined;
    },
    set(_, key, value) { element.setAttribute(datasetName(key), value); return true; },
    has(_, key) { return typeof key === "string" && element.hasAttribute(datasetName(key)); },
    deleteProperty(_, key) { element.removeAttribute(datasetName(key)); return true; },
    ownKeys() {
      return call("attributeNames", element[handle])
        .filter(name => name.startsWith("data-")).map(datasetKey);
    },
    getOwnPropertyDescriptor(_, key) {
      const value = typeof key === "string" ? element.getAttribute(datasetName(key)) : null;
      return value === null ? undefined : { value, writable: true, enumerable: true, configurable: true };
    },
  });

  // What the variadic insertion methods accept: anything that is not a node is
  // the text it stringifies to.
  const insertable = value => value instanceof Node ? value : document.createTextNode(String(value));


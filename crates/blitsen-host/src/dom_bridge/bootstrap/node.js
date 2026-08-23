  const NODE_TYPES = { element: 1, text: 3, comment: 8, document: 9, fragment: 11 };

  // The whole `nodeType` table, not only the kinds this engine builds: the
  // constants are a numbering that code compares against, so a document that
  // never holds a notation still has to say what a notation would have been.
  const NODE_TYPE_CONSTANTS = {
    ELEMENT_NODE: 1, ATTRIBUTE_NODE: 2, TEXT_NODE: 3, CDATA_SECTION_NODE: 4,
    ENTITY_REFERENCE_NODE: 5, ENTITY_NODE: 6, PROCESSING_INSTRUCTION_NODE: 7,
    COMMENT_NODE: 8, DOCUMENT_NODE: 9, DOCUMENT_TYPE_NODE: 10,
    DOCUMENT_FRAGMENT_NODE: 11, NOTATION_NODE: 12,
  };
  // Declared on the interface object *and* its prototype, which is where WebIDL
  // puts a constant and how `node.ELEMENT_NODE` resolves at all. Reading it off
  // the instance is ordinary: Monaco classifies a mouse target with
  // `child.nodeType === child.ELEMENT_NODE`, and an undefined right-hand side
  // made every click in the editor an unknown target (issue #129). Read-only and
  // non-configurable, as a browser writes them.
  const defineNodeTypeConstants = interface_ => {
    for (const [name, value] of Object.entries(NODE_TYPE_CONSTANTS)) {
      const constant = { value, writable: false, enumerable: true, configurable: false };
      Object.defineProperty(interface_, name, constant);
      Object.defineProperty(interface_.prototype, name, constant);
    }
  };

  const ancestorsOf = node => {
    const ancestors = [];
    for (let current = node; current; current = current.parentNode) ancestors.unshift(current);
    return ancestors;
  };
  // The order shared by node positions and range boundary points. `nodeBranch`
  // and `otherBranch` are the first children below the common ancestor; a null
  // branch means that node is the common ancestor itself. Different roots have
  // no tree order and return null.
  const treeOrder = (node, other) => {
    if (node === other) return { order: 0, nodeBranch: null, otherBranch: null };
    const mine = ancestorsOf(node);
    const theirs = ancestorsOf(other);
    if (mine[0] !== theirs[0]) return null;
    let depth = 0;
    while (mine[depth] === theirs[depth]) depth += 1;
    const nodeBranch = mine[depth] ?? null;
    const otherBranch = theirs[depth] ?? null;
    if (nodeBranch === null) return { order: -1, nodeBranch, otherBranch };
    if (otherBranch === null) return { order: 1, nodeBranch, otherBranch };
    const siblings = [...mine[depth - 1].childNodes];
    const order = siblings.indexOf(nodeBranch) < siblings.indexOf(otherBranch) ? -1 : 1;
    return { order, nodeBranch, otherBranch };
  };

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
    remove() { call("remove", this[handle]); windowModesTreeMutation(); }
    replaceWith(replacement) {
      call("replaceWith", this[handle], requireNode(replacement));
      windowModesTreeMutation();
    }
    // `replaceWith` from the parent's side, which is the form a renderer that
    // owns the parent reaches for — Monaco's line renderer is written entirely
    // in it. The parent has to be the child's own: replacing a node that lives
    // elsewhere would move the replacement out from under the caller, and a
    // browser refuses it rather than guessing.
    replaceChild(replacement, child) {
      const removed = requireNode(child);
      if (child.parentNode !== this) {
        throw new DOMException("the node to replace is not a child of this node", "NotFoundError");
      }
      const previousSibling = child.previousSibling;
      const nextSibling = child.nextSibling;
      if (replacement instanceof DocumentFragment) {
        insertFragment(this, replacement, child);
        return this.removeChild(child);
      }
      call("replaceWith", removed, requireNode(replacement));
      notifyMutation({ type: "childList", target: this, addedNodes: new NodeList([replacement]),
        removedNodes: new NodeList([child]), previousSibling, nextSibling });
      return child;
    }
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
    // Where `other` sits relative to this node, as the DOM's bitmask. Answered
    // by walking to the common ancestor and comparing child positions there,
    // which is the same order a tree walk would visit them in. Two nodes in
    // different trees are DISCONNECTED, and the direction reported for that
    // case is arbitrary but stable — hence IMPLEMENTATION_SPECIFIC alongside it,
    // exactly as a browser reports it.
    compareDocumentPosition(other) {
      if (!(other instanceof Node)) throw new TypeError("argument is not a Node");
      const relation = treeOrder(this, other);
      if (relation === null) return 1 + 2 + 32;
      if (relation.order === 0) return 0;
      // One chain running out first is the containment case: the shorter node is
      // the ancestor, and an ancestor precedes its descendant in document order.
      if (relation.nodeBranch === null) return 16 + 4;
      if (relation.otherBranch === null) return 8 + 2;
      return relation.order < 0 ? 4 : 2;
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

  defineNodeTypeConstants(Node);

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

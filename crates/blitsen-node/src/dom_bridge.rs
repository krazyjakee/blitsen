//! Native DOM object installation for the Bun host.

use std::cell::RefCell;
use std::rc::Rc;

use blitsen_blitz::BlitzDom;
use blitsen_core::{WindowState, WrapperTable, js_property_to_css};
use blitsen_dom::{DomBackend, DomError, DomName, Namespace, NodeKind};
use blitsen_js::{ExternalId, JsEngine, JsError, JsType, NativeClass, TypedArray, TypedArrayKind};
use blitz::dom::NodeId;
use napi::{Env, Unknown, sys};
use serde_json::{Value, json};

use super::{DomRuntime, NodeApiEngine, NodeWeakRef, callback_string, check, unknown};

mod fetch;
mod web_url;

const BOOTSTRAP: &str = r##"
(() => {
  const testHarness = Boolean(globalThis.__blitsenTestHarness);
  delete globalThis.__blitsenTestHarness;
  const hostSetTimeout = globalThis.setTimeout.bind(globalThis);
  const hostClearTimeout = globalThis.clearTimeout.bind(globalThis);
  const hostSetInterval = globalThis.setInterval.bind(globalThis);
  const hostClearInterval = globalThis.clearInterval.bind(globalThis);
  const contextTimeouts = new Set();
  const contextIntervals = new Set();
  const setTimeout = (callback, delay = 0, ...args) => {
    if (typeof callback !== "function") throw new TypeError("setTimeout callback must be a function");
    let id;
    id = hostSetTimeout((...values) => {
      contextTimeouts.delete(id);
      callback(...values);
    }, delay, ...args);
    contextTimeouts.add(id);
    return id;
  };
  const clearTimeout = id => {
    contextTimeouts.delete(id);
    hostClearTimeout(id);
  };
  const setInterval = (callback, delay = 0, ...args) => {
    if (typeof callback !== "function") throw new TypeError("setInterval callback must be a function");
    const id = hostSetInterval(callback, delay, ...args);
    contextIntervals.add(id);
    return id;
  };
  const clearInterval = id => {
    contextIntervals.delete(id);
    hostClearInterval(id);
  };
  const call = (operation, ...args) =>
    JSON.parse(__blitsenDomCall(operation, ...args.map(value => String(value))));
  const handle = Symbol("Blitsen node handle");
  let nextAnimationFrameId = 1;
  let animationFrames = new Map();
  let runningAnimationFrames = null;
  let forcedLayoutsThisFrame = 0;
  const recordForcedLayout = result => {
    if (result.forced) forcedLayoutsThisFrame++;
    return result;
  };

  const requestAnimationFrame = callback => {
    if (typeof callback !== "function") throw new TypeError("requestAnimationFrame callback must be a function");
    const id = nextAnimationFrameId++;
    animationFrames.set(id, callback);
    return id;
  };
  const cancelAnimationFrame = id => {
    animationFrames.delete(Number(id));
    runningAnimationFrames?.delete(Number(id));
  };
  const animationFrameTick = timestamp => {
    notifySurfaceResizes();
    notifyResizeObservers();
    notifyMediaQueries();
    settleFetches();
    settleImages();
    const callbacks = animationFrames;
    animationFrames = new Map();
    runningAnimationFrames = callbacks;
    for (const [id, callback] of callbacks) {
      if (!callbacks.has(id)) continue;
      try { callback(Number(timestamp)); }
      catch (error) { console.error("Uncaught exception in requestAnimationFrame callback", error); }
    }
    runningAnimationFrames = null;
    if (__blitsenDevLayoutWarnings && forcedLayoutsThisFrame > 0)
      console.warn(`Blitsen: ${forcedLayoutsThisFrame} forced synchronous layout(s) in this frame`);
    forcedLayoutsThisFrame = 0;
    // In-flight requests, undecoded images and undelivered resize observations
    // keep the host turning: their landing point is this function, so a loop
    // that stopped would never deliver them.
    return animationFrames.size + inflightFetches.size + pendingResizeObservations()
      + waitingImages();
  };

  const eventStates = new WeakMap();
  const stateFor = event => {
    const state = eventStates.get(event);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };

  class Event {
    constructor(type, options = {}) {
      eventStates.set(this, {
        type: String(type), target: null, currentTarget: null, eventPhase: 0,
        bubbles: Boolean(options.bubbles), cancelable: Boolean(options.cancelable),
        defaultPrevented: false, propagationStopped: false,
        immediatePropagationStopped: false, passive: false,
        dispatching: false, timeStamp: performance.now(),
      });
    }
    get type() { return stateFor(this).type; }
    get target() { return stateFor(this).target; }
    get currentTarget() { return stateFor(this).currentTarget; }
    get eventPhase() { return stateFor(this).eventPhase; }
    get bubbles() { return stateFor(this).bubbles; }
    get cancelable() { return stateFor(this).cancelable; }
    get defaultPrevented() { return stateFor(this).defaultPrevented; }
    get timeStamp() { return stateFor(this).timeStamp; }
    preventDefault() {
      const state = stateFor(this);
      if (state.cancelable && !state.passive) state.defaultPrevented = true;
    }
    stopPropagation() { stateFor(this).propagationStopped = true; }
    stopImmediatePropagation() {
      const state = stateFor(this);
      state.propagationStopped = true;
      state.immediatePropagationStopped = true;
    }
  }

  class MouseEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      const numbers = ["clientX", "clientY", "offsetX", "offsetY", "screenX", "screenY",
        "button", "buttons", "deltaX", "deltaY"];
      for (const property of numbers) Object.defineProperty(this, property, {
        value: Number(options[property] ?? 0), enumerable: true,
      });
      for (const property of ["ctrlKey", "shiftKey", "altKey", "metaKey"]) Object.defineProperty(this, property, {
        value: Boolean(options[property]), enumerable: true,
      });
    }
  }

  class KeyboardEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperties(this, {
        key: { value: String(options.key ?? ""), enumerable: true },
        code: { value: String(options.code ?? ""), enumerable: true },
        repeat: { value: Boolean(options.repeat), enumerable: true },
        ctrlKey: { value: Boolean(options.ctrlKey), enumerable: true },
        shiftKey: { value: Boolean(options.shiftKey), enumerable: true },
        altKey: { value: Boolean(options.altKey), enumerable: true },
        metaKey: { value: Boolean(options.metaKey), enumerable: true },
      });
    }
  }

  class CustomEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperty(this, "detail", { value: options.detail ?? null, enumerable: true });
    }
  }

  class SubmitEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperty(this, "submitter", { value: options.submitter ?? null, enumerable: true });
    }
  }

  const eventInternals = Object.freeze({
    state: stateFor,
    begin(event, target, currentTarget, eventPhase, passive = false) {
      const state = stateFor(event);
      state.target ??= target;
      state.currentTarget = currentTarget;
      state.eventPhase = eventPhase;
      state.passive = passive;
      return state;
    },
    finish(event) {
      const state = stateFor(event);
      state.currentTarget = null;
      state.eventPhase = 0;
      state.passive = false;
    },
  });

  const listenerMaps = new WeakMap();
  const listenersFor = target => {
    let map = listenerMaps.get(target);
    if (!map) { map = new Map(); listenerMaps.set(target, map); }
    return map;
  };
  const listenerOptions = options => typeof options === "boolean"
    ? { capture: options, once: false, passive: false }
    : { capture: Boolean(options?.capture), once: Boolean(options?.once), passive: Boolean(options?.passive) };
  const validListener = callback => typeof callback === "function" ||
    (callback !== null && typeof callback === "object" && typeof callback.handleEvent === "function");
  const callListener = (callback, currentTarget, event) => typeof callback === "function"
    ? callback.call(currentTarget, event)
    : callback.handleEvent.call(callback, event);

  const removeListenerRecord = (target, type, record) => {
    record.removed = true;
    const listeners = listenerMaps.get(target)?.get(type);
    if (!listeners) return;
    const index = listeners.indexOf(record);
    if (index >= 0) listeners.splice(index, 1);
  };

  const invokeListenerSnapshot = (target, event, phase, capture, snapshot) => {
    const state = stateFor(event);
    state.currentTarget = target;
    state.eventPhase = phase;
    for (const record of snapshot) {
      if (state.immediatePropagationStopped) break;
      if (record.removed || record.capture !== capture) continue;
      if (record.once) removeListenerRecord(target, state.type, record);
      state.passive = record.passive;
      try { callListener(record.callback, target, event); }
      catch (error) { console.error("Uncaught exception in event listener", error); }
      finally { state.passive = false; }
    }
  };

  const propagationPath = target => {
    if (target === globalThis) return [globalThis];
    if (target === document) return [globalThis, document];
    if (!(target instanceof Node)) return [target];
    const ancestors = [];
    for (let parent = target.parentNode; parent instanceof Element; parent = parent.parentNode)
      ancestors.push(parent);
    return target.isConnected
      ? [globalThis, document, ...ancestors.reverse(), target]
      : [...ancestors.reverse(), target];
  };

  const dispatchTo = (target, event) => {
    if (!(event instanceof Event)) throw new TypeError("dispatchEvent argument must be an Event");
    const state = stateFor(event);
    if (state.dispatching) throw new DOMException("The event is already being dispatched", "InvalidStateError");
    state.dispatching = true;
    state.target = target;
    state.propagationStopped = false;
    state.immediatePropagationStopped = false;
    const path = propagationPath(target);
    try {
      for (const currentTarget of path.slice(0, -1)) {
        const snapshot = [...(listenerMaps.get(currentTarget)?.get(state.type) ?? [])];
        invokeListenerSnapshot(currentTarget, event, 1, true, snapshot);
        if (state.propagationStopped) return !state.defaultPrevented;
      }
      const snapshot = [...(listenerMaps.get(target)?.get(state.type) ?? [])];
      invokeListenerSnapshot(target, event, 2, true, snapshot);
      invokeListenerSnapshot(target, event, 2, false, snapshot);
      if (state.bubbles && !state.propagationStopped) {
        for (const currentTarget of path.slice(0, -1).reverse()) {
          const listeners = [...(listenerMaps.get(currentTarget)?.get(state.type) ?? [])];
          invokeListenerSnapshot(currentTarget, event, 3, false, listeners);
          if (state.propagationStopped) break;
        }
      }
      return !state.defaultPrevented;
    } finally {
      state.dispatching = false;
      eventInternals.finish(event);
    }
  };

  let activeElement = null;
  let readyState = "loading";
  const elementTag = element => call("tagName", element[handle]);
  const isFocusable = element => {
    if (!(element instanceof Element) || element.hasAttribute("disabled")) return false;
    const tabindex = element.getAttribute("tabindex");
    if (tabindex !== null) return Number(tabindex) >= 0;
    const tag = elementTag(element);
    return ["button", "input", "select", "textarea"].includes(tag) ||
      (tag === "a" && element.hasAttribute("href"));
  };
  const setFocus = element => {
    const next = element ?? document.body;
    const previous = activeElement ?? document.body;
    if (next === previous) { activeElement = next; return; }
    activeElement = next;
    previous?.dispatchEvent(new Event("blur"));
    next?.dispatchEvent(new Event("focus"));
  };
  const focusNearest = target => {
    for (let element = target; element instanceof Element; element = element.parentNode)
      if (isFocusable(element)) { setFocus(element); return; }
    setFocus(document.body);
  };
  const moveFocus = backwards => {
    const focusables = [...document.querySelectorAll("*")].filter(isFocusable);
    if (focusables.length === 0) { setFocus(document.body); return; }
    const current = focusables.indexOf(activeElement);
    const index = backwards
      ? (current <= 0 ? focusables.length - 1 : current - 1)
      : (current < 0 || current === focusables.length - 1 ? 0 : current + 1);
    setFocus(focusables[index]);
  };
  const dispatchMouseEvent = (type, rawHandle, init) => {
    const target = wrap(String(rawHandle));
    const event = new MouseEvent(String(type), init);
    const allowed = target.dispatchEvent(event);
    if (type === "click" && allowed) { focusNearest(target); activateControl(target); }
    if (type === "wheel" && allowed)
      __blitsenScrollDefault(String(target[handle]), String(-event.deltaX), String(-event.deltaY));
    return allowed;
  };
  const dispatchKeyboardEvent = (type, init) => {
    const event = new KeyboardEvent(String(type), init);
    const target = activeElement ?? document.body ?? document;
    const allowed = target.dispatchEvent(event);
    if (type === "keydown" && init.key === "Tab" && allowed) moveFocus(Boolean(init.shiftKey));
    if (type === "keydown" && allowed && target instanceof Node && !event.ctrlKey && !event.altKey && !event.metaKey) {
      const page = Math.max(1, innerHeight * 0.9);
      const delta = {
        ArrowLeft: [-40, 0], ArrowRight: [40, 0], ArrowUp: [0, -40], ArrowDown: [0, 40],
        PageUp: [0, -page], PageDown: [0, page], Home: [0, -1e9], End: [0, 1e9],
        " ": [0, event.shiftKey ? -page : page],
      }[event.key];
      if (delta) __blitsenScrollDefault(String(target[handle]), String(-delta[0]), String(-delta[1]));
    }
    return allowed;
  };
  const dispatchLifecycleEvent = type => {
    if (type === "DOMContentLoaded") {
      readyState = "interactive";
      return document.dispatchEvent(new Event(type, { bubbles: true }));
    }
    if (type === "load") {
      readyState = "complete";
      return globalThis.dispatchEvent(new Event(type));
    }
    return globalThis.dispatchEvent(new Event(type));
  };

  class EventTarget {
    addEventListener(type, callback, options = false) {
      if (!validListener(callback)) return;
      type = String(type);
      const normalized = listenerOptions(options);
      const map = listenersFor(this);
      const listeners = map.get(type) ?? [];
      if (listeners.some(record => !record.removed && record.callback === callback && record.capture === normalized.capture))
        return;
      listeners.push({ callback, ...normalized, removed: false });
      map.set(type, listeners);
    }
    removeEventListener(type, callback, options = false) {
      if (!validListener(callback)) return;
      type = String(type);
      const capture = listenerOptions(options).capture;
      const record = listenerMaps.get(this)?.get(type)?.find(record =>
        !record.removed && record.callback === callback && record.capture === capture);
      if (record) removeListenerRecord(this, type, record);
    }
    dispatchEvent(event) { return dispatchTo(this, event); }
  }

  const mutationObservers = new Set();
  const isObservedTarget = (observed, target, subtree) => {
    if (observed === target) return true;
    if (!subtree) return false;
    for (let ancestor = target?.parentNode; ancestor; ancestor = ancestor.parentNode)
      if (ancestor === observed) return true;
    return false;
  };
  const notifyMutation = record => {
    for (const observer of mutationObservers) {
      if (!observer._observations.some(({ target, options }) =>
        options[record.type] && isObservedTarget(target, record.target, options.subtree))) continue;
      observer._records.push(Object.freeze(record));
      if (observer._queued) continue;
      observer._queued = true;
      queueMicrotask(() => {
        observer._queued = false;
        const records = observer.takeRecords();
        if (records.length > 0 && observer._observations.length > 0)
          observer._callback(records, observer);
      });
    }
  };

  class MutationObserver {
    constructor(callback) {
      if (typeof callback !== "function") throw new TypeError("MutationObserver callback must be a function");
      this._callback = callback;
      this._observations = [];
      this._records = [];
      this._queued = false;
    }
    observe(target, options = {}) {
      if (!(target instanceof Node) && target !== document)
        throw new TypeError("MutationObserver target must be a Node");
      const normalized = {
        childList: Boolean(options.childList), attributes: Boolean(options.attributes),
        characterData: Boolean(options.characterData), subtree: Boolean(options.subtree),
      };
      if (!normalized.childList && !normalized.attributes && !normalized.characterData)
        throw new TypeError("MutationObserver options must enable at least one mutation type");
      this._observations = this._observations.filter(observation => observation.target !== target);
      this._observations.push({ target, options: normalized });
      mutationObservers.add(this);
    }
    disconnect() {
      this._observations = [];
      this._records = [];
      mutationObservers.delete(this);
    }
    takeRecords() { return this._records.splice(0); }
  }

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

  class Element extends Node {
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
    // Blitz lays an element out as one box: there is no fragmentation across
    // columns or line boxes to report, so the list is the border box and the
    // same layout read `getBoundingClientRect` is.
    getClientRects() { return Object.freeze([this.getBoundingClientRect()]); }
    getBoundingClientRect() {
      const { x, y, width, height } = recordForcedLayout(call("layoutMetrics", this[handle]));
      return Object.freeze({
        x, y, width, height, top: y, right: x + width, bottom: y + height, left: x,
        toJSON() { return { x, y, width, height, top: y, right: x + width, bottom: y + height, left: x }; },
      });
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
  }

  // Images. Blitz decodes subresources beside the DOM and announces nothing when
  // one lands, so `load` and `error` are delivered by polling the elements that
  // owe an outcome — at the frame boundary, where `fetch` completions land too.
  const pendingImages = new Set();
  const imageHandlers = new WeakMap();
  const imageState = element => call("imageState", element[handle]);
  const setImageHandler = (element, type, callback) => {
    let handlers = imageHandlers.get(element);
    if (!handlers) imageHandlers.set(element, handlers = { load: null, error: null });
    if (handlers[type]) element.removeEventListener(type, handlers[type]);
    handlers[type] = typeof callback === "function" ? callback : null;
    if (handlers[type]) element.addEventListener(type, handlers[type]);
  };
  // Over a copy: a handler that gives another image a source owes that outcome
  // to the next frame, not to the rest of this pass.
  const settleImages = () => {
    for (const element of [...pendingImages]) {
      const state = imageState(element);
      if (!state.complete) continue;
      pendingImages.delete(element);
      element.dispatchEvent(new Event(state.errored ? "error" : "load"));
    }
  };
  // Blitz requests a source only once the element is in the document, so a
  // detached image is waiting on nothing and must not hold the host open.
  const waitingImages = () => {
    let waiting = 0;
    for (const element of pendingImages) if (element.isConnected) waiting++;
    return waiting;
  };

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
      Object.defineProperty(this, "length", { value: items.length, enumerable: false });
      items.forEach((item, index) => Object.defineProperty(this, index, { value: item, enumerable: true }));
      Object.freeze(this);
    }
    item(index) { return this[index] ?? null; }
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
      Object.defineProperty(this, "length", { value: nodes.length, enumerable: false });
      nodes.forEach((node, index) => Object.defineProperty(this, index, { value: node, enumerable: true }));
      Object.freeze(this);
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

  class CSSStyleDeclaration {
    constructor(element) { this._element = element; }
    _name(property) { const name = String(property); return name.startsWith("--") ? name : name.toLowerCase(); }
    getPropertyValue(property) { return call("styleGet", this._element[handle], this._name(property)); }
    setProperty(property, value) { call("styleSet", this._element[handle], this._name(property), String(value)); }
    removeProperty(property) { return call("styleRemove", this._element[handle], this._name(property)); }
    get cssText() { return call("styleText", this._element[handle]); }
    set cssText(value) { call("setStyleText", this._element[handle], String(value)); }
    _getJsProperty(property) { return call("styleGetJs", this._element[handle], property); }
    _setJsProperty(property, value) { call("styleSetJs", this._element[handle], property, value); }
  }

  // Computed style. Blitz has already resolved the cascade, so this reads that
  // answer back rather than keeping a second idea of what an element's style is.
  // Every read is layout-dependent — `width` and `height` resolve to the used
  // value — so it takes the same flush the geometry reads take, and a read after
  // a write counts as the forced layout it is.
  const readOnlyStyle = () => {
    throw new DOMException("a computed style declaration is read-only", "NoModificationAllowedError");
  };

  class CSSResolvedStyleDeclaration extends CSSStyleDeclaration {
    // An empty string here is what a browser returns too: an unknown property,
    // an unset custom property, or a shorthand whose longhands do not compose.
    // The one case a browser answers differently is an element the cascade has
    // never reached — see COMPATIBILITY.md.
    getPropertyValue(property) {
      return recordForcedLayout(
        call("computedStyle", this._element[handle], this._name(property))).value ?? "";
    }
    // CSSOM: a computed declaration block serializes as nothing.
    get cssText() { return ""; }
    set cssText(value) { readOnlyStyle(); }
    setProperty(property, value) { readOnlyStyle(); }
    removeProperty(property) { readOnlyStyle(); }
    _getJsProperty(property) {
      return recordForcedLayout(
        call("computedStyleJs", this._element[handle], property)).value ?? "";
    }
    _setJsProperty(property, value) { readOnlyStyle(); }
  }

  const computedStyleCache = new WeakMap();
  const getComputedStyle = (element, pseudoElement = null) => {
    if (!(element instanceof Element)) throw new TypeError("getComputedStyle requires an Element");
    // A pseudo-element box is not addressable through this bridge, and answering
    // with the originating element's style would be a wrong answer rather than
    // a missing one.
    if (pseudoElement != null && String(pseudoElement) !== "")
      throw new DOMException(`no resolved style for ${pseudoElement}`, "NotSupportedError");
    let style = computedStyleCache.get(element);
    if (!style) {
      style = new Proxy(new CSSResolvedStyleDeclaration(element), {
        get(target, property, receiver) {
          if (typeof property !== "string" || property in target) return Reflect.get(target, property, receiver);
          return target._getJsProperty(property);
        },
        set() { readOnlyStyle(); },
      });
      computedStyleCache.set(element, style);
    }
    return style;
  };

  // Media queries. Stylo evaluates `@media` for the cascade; this asks it the
  // same question from JavaScript, so a feature the style engine does not
  // implement is unknown to both and its query does not match.
  const mediaQueryLists = new Set();
  const mediaStates = new WeakMap();
  const mediaStateFor = list => {
    const state = mediaStates.get(list);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };

  class MediaQueryListEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperties(this, {
        media: { value: String(options.media ?? ""), enumerable: true },
        matches: { value: Boolean(options.matches), enumerable: true },
      });
    }
  }

  class MediaQueryList extends EventTarget {
    constructor() { throw new TypeError("Illegal constructor"); }
    get media() { return mediaStateFor(this).media; }
    get matches() { return mediaStateFor(this).matches; }
    get onchange() { return mediaStateFor(this).onchange; }
    set onchange(callback) {
      const state = mediaStateFor(this);
      if (state.onchange) this.removeEventListener("change", state.onchange);
      state.onchange = typeof callback === "function" ? callback : null;
      if (state.onchange) this.addEventListener("change", state.onchange);
    }
    // A list is only worth re-evaluating once something is listening to it.
    addEventListener(type, callback, options = false) {
      super.addEventListener(type, callback, options);
      mediaQueryLists.add(this);
    }
    // The pre-2019 spelling, which a library still installs when its own type
    // definitions predate `addEventListener` on this interface.
    addListener(callback) { this.addEventListener("change", callback); }
    removeListener(callback) { this.removeEventListener("change", callback); }
  }

  const matchMedia = query => {
    query = String(query);
    const list = Object.create(MediaQueryList.prototype);
    mediaStates.set(list, { query, onchange: null, ...call("matchMedia", query) });
    return list;
  };
  // The only device state an exported application can change is the viewport:
  // the colour scheme is fixed for the life of the process, so a query can only
  // flip when the window does.
  let mediaViewport = null;
  const notifyMediaQueries = () => {
    const viewport = `${innerWidth}x${innerHeight}@${devicePixelRatio}`;
    if (viewport === mediaViewport) return;
    mediaViewport = viewport;
    for (const list of mediaQueryLists) {
      const state = mediaStateFor(list);
      const { matches } = call("matchMedia", state.query);
      if (matches === state.matches) continue;
      state.matches = matches;
      list.dispatchEvent(new MediaQueryListEvent("change", { media: state.media, matches }));
    }
  };

  // Element resize observation, delivered at the top of the frame turn beside
  // the surface resizes, which is where this runtime settles geometry.
  const resizeObservers = new Set();
  const resizeSignature = (metrics, box) => box === "border-box"
    ? `${metrics.width}x${metrics.height}`
    : `${metrics.contentWidth}x${metrics.contentHeight}`;
  const resizeEntry = (target, metrics) => {
    const { contentX: x, contentY: y, contentWidth: width, contentHeight: height } = metrics;
    return Object.freeze({
      target,
      contentRect: Object.freeze({ x, y, width, height,
        top: y, right: x + width, bottom: y + height, left: x }),
      // Physical writing modes only: inline is width and block is height, which
      // holds for every writing mode this renderer lays out.
      borderBoxSize: Object.freeze([
        Object.freeze({ inlineSize: metrics.width, blockSize: metrics.height })]),
      contentBoxSize: Object.freeze([Object.freeze({ inlineSize: width, blockSize: height })]),
    });
  };
  // An element that has never been reported is work the frame loop owes the
  // application, the way an in-flight request is.
  const pendingResizeObservations = () => {
    let pending = 0;
    for (const observer of resizeObservers)
      for (const record of observer._targets.values()) if (record.reported === null) pending++;
    return pending;
  };
  const notifyResizeObservers = () => {
    for (const observer of resizeObservers) {
      const entries = [];
      for (const [target, record] of observer._targets) {
        if (!target.isConnected) continue;
        const metrics = call("layoutMetrics", target[handle]);
        const signature = resizeSignature(metrics, record.box);
        if (signature === record.reported) continue;
        record.reported = signature;
        entries.push(resizeEntry(target, metrics));
      }
      if (entries.length === 0) continue;
      try { observer._callback(entries, observer); }
      catch (error) { console.error("Uncaught exception in ResizeObserver callback", error); }
    }
  };

  class ResizeObserver {
    constructor(callback) {
      if (typeof callback !== "function") throw new TypeError("ResizeObserver callback must be a function");
      this._callback = callback;
      this._targets = new Map();
    }
    observe(target, options = {}) {
      if (!(target instanceof Element)) throw new TypeError("ResizeObserver target must be an Element");
      const box = String(options.box ?? "content-box");
      // `device-pixel-content-box` needs a device-pixel snap this bridge does
      // not report, so it is refused rather than answered in CSS pixels.
      if (box !== "content-box" && box !== "border-box")
        throw new TypeError(`unsupported ResizeObserver box: ${box}`);
      this._targets.set(target, { box, reported: null });
      resizeObservers.add(this);
    }
    unobserve(target) { this._targets.delete(target); }
    disconnect() { this._targets.clear(); resizeObservers.delete(this); }
  }

  // Form controls. The attribute is the control's *default* and the property is
  // its current state: HTML calls the divergence the dirty value flag, and
  // getting it backwards would look like it worked. `value` and `checked` read
  // and write the state the renderer paints from — there is no second store
  // here that could disagree with the pixels — while `defaultValue` and
  // `defaultChecked` are the attribute reflections.
  const INPUT_TYPES = ["button", "checkbox", "color", "date", "datetime-local", "email", "file",
    "hidden", "image", "month", "number", "password", "radio", "range", "reset", "search",
    "submit", "tel", "text", "time", "url", "week"];
  // The types whose value is control state rather than the attribute. The rest
  // are HTML's default mode: `value` is the attribute and nothing else.
  const VALUE_TYPES = ["color", "date", "datetime-local", "email", "month", "number", "password",
    "range", "search", "tel", "text", "time", "url", "week"];
  const CHECKABLE_TYPES = ["checkbox", "radio"];
  const SUBMIT_TYPES = ["submit", "image"];
  // What `form.elements` lists, minus the form-associated custom elements this
  // runtime has no custom elements to have.
  const FORM_CONTROLS = "button, fieldset, input, object, output, select, textarea";
  const reflected = (element, name) => element.getAttribute(name) ?? "";
  const controlValue = element => call("formValue", element[handle]);
  const setControlValue = (element, value) => call("setFormValue", element[handle], value);
  const controlChecked = element => call("formChecked", element[handle]);
  const setControlChecked = (element, checked) => call("setFormChecked", element[handle], checked);
  // The form owner: an explicit `form` attribute naming one, else the ancestor.
  const formOwner = element => {
    const named = element.getAttribute("form");
    if (named === null) return element.closest("form");
    const owner = document.getElementById(named);
    return owner !== null && elementTag(owner) === "form" ? owner : null;
  };
  const listedControls = form =>
    [...document.querySelectorAll(FORM_CONTROLS)].filter(control => formOwner(control) === form);
  const options = select => [...select.querySelectorAll("option")];
  const isSubmitButton = element => {
    const type = (element.getAttribute("type") ?? "").toLowerCase();
    if (elementTag(element) === "button") return type === "" || type === "submit";
    return elementTag(element) === "input" && SUBMIT_TYPES.includes(type);
  };
  // A radio group has one member checked at a time, which is what makes it a
  // group: the siblings are written here rather than left disagreeing with what
  // is painted.
  const setChecked = (input, checked) => {
    setControlChecked(input, checked);
    if (!checked || input.type !== "radio") return;
    const name = input.getAttribute("name");
    if (!name) return;
    const owner = formOwner(input);
    for (const other of document.querySelectorAll('input[type="radio"]'))
      if (other !== input && other.getAttribute("name") === name && formOwner(other) === owner)
        setControlChecked(other, false);
  };
  const setSelected = (option, selected) => {
    setControlChecked(option, selected);
    const select = option.closest("select");
    if (!selected || select === null || select.multiple) return;
    for (const other of options(select)) if (other !== option) setControlChecked(other, false);
  };
  // There is nowhere to navigate to, so submission is the event and nothing
  // else — which is the half a single-page application actually uses, and the
  // half it can cancel. See COMPATIBILITY.md for why `submit()` is absent.
  const submitForm = (form, submitter) =>
    form.dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true, submitter }));
  // The activation behaviour a control has of its own, run after the click and
  // only when the click was not cancelled — which is what makes preventDefault
  // on a checkbox or a submit button mean anything.
  const activateControl = target => {
    for (let element = target; element instanceof Element; element = element.parentNode) {
      if (element.hasAttribute("disabled")) return;
      if (elementTag(element) === "input" && CHECKABLE_TYPES.includes(element.type)) {
        if (element.type === "radio" && element.checked) return;
        setChecked(element, element.type === "radio" || !element.checked);
        element.dispatchEvent(new Event("input", { bubbles: true }));
        element.dispatchEvent(new Event("change", { bubbles: true }));
        return;
      }
      if (isSubmitButton(element)) {
        const form = formOwner(element);
        if (form !== null) submitForm(form, element);
        return;
      }
    }
  };

  class HTMLFormControlElement extends Element {
    get name() { return reflected(this, "name"); }
    set name(value) { this.setAttribute("name", value); }
    get disabled() { return this.hasAttribute("disabled"); }
    set disabled(value) { this.toggleAttribute("disabled", Boolean(value)); }
    get form() { return formOwner(this); }
  }

  class HTMLInputElement extends HTMLFormControlElement {
    get type() {
      const type = (this.getAttribute("type") ?? "").toLowerCase();
      return INPUT_TYPES.includes(type) ? type : "text";
    }
    set type(value) { this.setAttribute("type", value); }
    get value() {
      if (VALUE_TYPES.includes(this.type)) return controlValue(this);
      // A checkbox submits "on" when it carries no value of its own.
      return this.getAttribute("value") ?? (CHECKABLE_TYPES.includes(this.type) ? "on" : "");
    }
    set value(value) {
      value = value === null ? "" : String(value);
      if (VALUE_TYPES.includes(this.type)) setControlValue(this, value);
      else this.setAttribute("value", value);
    }
    get defaultValue() { return reflected(this, "value"); }
    set defaultValue(value) { this.setAttribute("value", value); }
    get checked() { return controlChecked(this); }
    set checked(value) { setChecked(this, Boolean(value)); }
    get defaultChecked() { return this.hasAttribute("checked"); }
    set defaultChecked(value) { this.toggleAttribute("checked", Boolean(value)); }
  }

  class HTMLTextAreaElement extends HTMLFormControlElement {
    get type() { return "textarea"; }
    get value() { return controlValue(this); }
    set value(value) { setControlValue(this, value === null ? "" : String(value)); }
    // A textarea's child text is its default value, where an input has an
    // attribute; the renderer is given it too, so an untouched textarea paints
    // what it reads.
    get defaultValue() { return this.textContent; }
    set defaultValue(value) { this.textContent = value; }
  }

  class HTMLButtonElement extends HTMLFormControlElement {
    get type() {
      const type = (this.getAttribute("type") ?? "").toLowerCase();
      return type === "reset" || type === "button" ? type : "submit";
    }
    set type(value) { this.setAttribute("type", value); }
    get value() { return reflected(this, "value"); }
    set value(value) { this.setAttribute("value", value); }
  }

  class HTMLOptionElement extends Element {
    // Falling back to the text is the whole of what an option without a value
    // attribute submits.
    get value() { return this.getAttribute("value") ?? this.text; }
    set value(value) { this.setAttribute("value", value); }
    get text() { return this.textContent.replace(/\s+/g, " ").trim(); }
    set text(value) { this.textContent = value; }
    get label() { return this.getAttribute("label") ?? this.text; }
    set label(value) { this.setAttribute("label", value); }
    get selected() { return controlChecked(this); }
    set selected(value) { setSelected(this, Boolean(value)); }
    get defaultSelected() { return this.hasAttribute("selected"); }
    set defaultSelected(value) { this.toggleAttribute("selected", Boolean(value)); }
    get disabled() { return this.hasAttribute("disabled"); }
    set disabled(value) { this.toggleAttribute("disabled", Boolean(value)); }
    get index() {
      const select = this.closest("select");
      return select === null ? 0 : options(select).indexOf(this);
    }
    get form() {
      const select = this.closest("select");
      return select === null ? null : formOwner(select);
    }
  }

  class HTMLSelectElement extends HTMLFormControlElement {
    get type() { return this.multiple ? "select-multiple" : "select-one"; }
    get multiple() { return this.hasAttribute("multiple"); }
    set multiple(value) { this.toggleAttribute("multiple", Boolean(value)); }
    get size() { return Number(this.getAttribute("size")) || 0; }
    // Static, as every collection this runtime hands out is: a re-read sees the
    // options added since, the collection handed out before it does not.
    get options() { return new NodeList(options(this)); }
    get length() { return options(this).length; }
    get selectedOptions() { return new NodeList(options(this).filter(option => option.selected)); }
    // A drop-down always shows something, so one with nothing selected reports
    // its first enabled option rather than -1. That is the selectedness HTML
    // resets a drop-down to; what it does not do is stay at -1 after an
    // assignment that matched nothing. See COMPATIBILITY.md.
    get selectedIndex() {
      const list = options(this);
      const selected = list.findIndex(option => option.selected);
      if (selected >= 0 || this.multiple || this.size > 1) return selected;
      return list.findIndex(option => !option.disabled);
    }
    set selectedIndex(index) {
      index = Number(index);
      options(this).forEach((option, position) => setControlChecked(option, position === index));
    }
    get value() {
      const index = this.selectedIndex;
      return index < 0 ? "" : options(this)[index].value;
    }
    set value(value) {
      value = String(value);
      const list = options(this);
      const index = list.findIndex(option => option.value === value);
      list.forEach((option, position) => setControlChecked(option, position === index));
    }
  }

  class HTMLFormElement extends Element {
    get name() { return reflected(this, "name"); }
    set name(value) { this.setAttribute("name", value); }
    // Static, like every other collection here.
    get elements() { return new NodeList(listedControls(this)); }
    get length() { return listedControls(this).length; }
    requestSubmit(submitter = null) {
      if (submitter !== null) {
        if (!(submitter instanceof Element) || !isSubmitButton(submitter))
          throw new TypeError("the submitter must be a submit button");
        if (formOwner(submitter) !== this)
          throw new DOMException("the submitter does not belong to this form", "NotFoundError");
      }
      submitForm(this, submitter);
    }
  }

  const requireNode = value => {
    if (!(value instanceof Node) || !(handle in value)) throw new TypeError("argument is not a Node");
    return value[handle];
  };
  const wrapperCache = new Map();
  const TAG_INTERFACES = { "blitsen-view": BlitsenViewElement, button: HTMLButtonElement,
    form: HTMLFormElement, img: HTMLImageElement, input: HTMLInputElement, link: HTMLLinkElement,
    option: HTMLOptionElement, select: HTMLSelectElement, template: HTMLTemplateElement,
    textarea: HTMLTextAreaElement };
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
  // Networking. Blitsen's own fetch rather than the host's, so the Phase 2
  // engine swap is invisible to the application. There is no same-origin policy
  // and no CORS: an exported application is trusted native software, not a
  // document. Bodies are buffered — see COMPATIBILITY.md for why streaming is
  // not in this tier.
  const HEADER_NAME = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  const headerFields = new WeakMap();
  const fieldsFor = headers => {
    const fields = headerFields.get(headers);
    if (!fields) throw new TypeError("Illegal invocation");
    return fields;
  };

  class Headers {
    constructor(init) {
      headerFields.set(this, new Map());
      if (init === undefined || init === null) return;
      if (init instanceof Headers || Array.isArray(init)) {
        for (const pair of init) {
          if (!Array.isArray(pair) || pair.length !== 2)
            throw new TypeError("Headers entries must be [name, value] pairs");
          this.append(pair[0], pair[1]);
        }
        return;
      }
      if (typeof init !== "object") throw new TypeError("invalid Headers initializer");
      for (const name of Object.keys(init)) this.append(name, init[name]);
    }
    _name(name) {
      const key = String(name).toLowerCase();
      if (!HEADER_NAME.test(key)) throw new TypeError(`invalid header name: ${name}`);
      return key;
    }
    append(name, value) {
      const key = this._name(name);
      const fields = fieldsFor(this);
      const next = String(value).trim();
      const existing = fields.get(key);
      fields.set(key, existing === undefined ? next : `${existing}, ${next}`);
    }
    set(name, value) { fieldsFor(this).set(this._name(name), String(value).trim()); }
    get(name) { return fieldsFor(this).get(this._name(name)) ?? null; }
    has(name) { return fieldsFor(this).has(this._name(name)); }
    delete(name) { fieldsFor(this).delete(this._name(name)); }
    forEach(callback, thisArg) { for (const [name, value] of this) callback.call(thisArg, value, name, this); }
    *entries() {
      const fields = fieldsFor(this);
      for (const name of [...fields.keys()].sort()) yield [name, fields.get(name)];
    }
    *keys() { for (const [name] of this) yield name; }
    *values() { for (const [, value] of this) yield value; }
    [Symbol.iterator]() { return this.entries(); }
  }

  const blobBytes = new WeakMap();
  const bytesOf = blob => {
    const bytes = blobBytes.get(blob);
    if (!bytes) throw new TypeError("Illegal invocation");
    return bytes;
  };
  const concatBytes = chunks => {
    const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    return bytes;
  };
  const asBytes = value => {
    if (typeof value === "string") return __blitsenUtf8Encode(value);
    if (value instanceof Blob) return bytesOf(value);
    if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength));
    if (value instanceof ArrayBuffer) return new Uint8Array(value.slice(0));
    return null;
  };

  class Blob {
    constructor(parts = [], options = {}) {
      blobBytes.set(this, concatBytes([...parts].map(part => asBytes(part) ?? __blitsenUtf8Encode(String(part)))));
      Object.defineProperty(this, "type", { value: String(options.type ?? "").toLowerCase(), enumerable: true });
    }
    get size() { return bytesOf(this).length; }
    slice(start, end, type) { return new Blob([bytesOf(this).slice(start, end)], { type: type ?? "" }); }
    text() { return Promise.resolve(__blitsenUtf8Decode(bytesOf(this))); }
    arrayBuffer() { return Promise.resolve(bytesOf(this).slice().buffer); }
  }

  const signalStates = new WeakMap();
  const signalState = signal => {
    const state = signalStates.get(signal);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };
  const createSignal = () => {
    const signal = Object.create(AbortSignal.prototype);
    signalStates.set(signal, { aborted: false, reason: undefined, onabort: null });
    return signal;
  };
  const raiseAbort = (signal, reason) => {
    const state = signalState(signal);
    if (state.aborted) return;
    state.aborted = true;
    state.reason = reason ?? new DOMException("The operation was aborted", "AbortError");
    signal.dispatchEvent(new Event("abort"));
  };

  class AbortSignal extends EventTarget {
    constructor() { throw new TypeError("Illegal constructor"); }
    get aborted() { return signalState(this).aborted; }
    get reason() { return signalState(this).reason; }
    get onabort() { return signalState(this).onabort; }
    set onabort(callback) {
      const state = signalState(this);
      if (state.onabort) this.removeEventListener("abort", state.onabort);
      state.onabort = typeof callback === "function" ? callback : null;
      if (state.onabort) this.addEventListener("abort", state.onabort);
    }
    throwIfAborted() { const state = signalState(this); if (state.aborted) throw state.reason; }
    static abort(reason) { const signal = createSignal(); raiseAbort(signal, reason); return signal; }
    static timeout(milliseconds) {
      const signal = createSignal();
      setTimeout(() => raiseAbort(signal, new DOMException("The operation timed out", "TimeoutError")),
        Number(milliseconds));
      return signal;
    }
  }

  class AbortController {
    constructor() { Object.defineProperty(this, "signal", { value: createSignal(), enumerable: true }); }
    abort(reason) { raiseAbort(this.signal, reason); }
  }

  const bodyStates = new WeakMap();
  const bodyStateFor = target => {
    const state = bodyStates.get(target);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };
  // A body the application never reads still occupies Rust memory, and only the
  // collector knows the Response was abandoned.
  const abandonedBodies = new FinalizationRegistry(id => __blitsenFetchCancel(String(id)));
  const readBody = (target, kind) => {
    const state = bodyStateFor(target);
    if (state.used) return Promise.reject(new TypeError("the body has already been read"));
    state.used = true;
    try {
      if (state.id === null) {
        const bytes = state.bytes ?? new Uint8Array(0);
        return Promise.resolve(kind === "text" ? __blitsenUtf8Decode(bytes) : bytes);
      }
      const id = state.id;
      state.id = null;
      abandonedBodies.unregister(target);
      return Promise.resolve(__blitsenFetchBody(String(id), kind));
    } catch (error) {
      return Promise.reject(error);
    }
  };
  const installBodyMethods = prototype => Object.defineProperties(prototype, {
    bodyUsed: { get() { return bodyStateFor(this).used; } },
    text: { value() { return readBody(this, "text"); } },
    json: { value() { return readBody(this, "text").then(text => JSON.parse(text)); } },
    arrayBuffer: { value() { return readBody(this, "bytes").then(bytes => bytes.buffer); } },
    blob: { value() {
      return readBody(this, "bytes").then(bytes => new Blob([bytes], { type: this.headers.get("content-type") ?? "" }));
    } },
  });

  const KNOWN_METHODS = ["DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"];
  const normalizeMethod = method => {
    const value = String(method);
    const upper = value.toUpperCase();
    return KNOWN_METHODS.includes(upper) ? upper : value;
  };
  const encodeBody = (body, headers) => {
    if (body === undefined || body === null) return null;
    const bytes = asBytes(body);
    if (bytes === null)
      throw new TypeError("a fetch body must be a string, Blob, ArrayBuffer, or typed array");
    if (!headers.has("content-type")) {
      if (typeof body === "string") headers.set("content-type", "text/plain;charset=UTF-8");
      else if (body instanceof Blob && body.type) headers.set("content-type", body.type);
    }
    return body instanceof Blob ? bytes.slice() : bytes;
  };

  class Request {
    constructor(input, options = {}) {
      const source = input instanceof Request ? input : null;
      const headers = new Headers(options.headers ?? source?.headers);
      const method = normalizeMethod(options.method ?? source?.method ?? "GET");
      const body = "body" in options
        ? encodeBody(options.body, headers)
        : source ? bodyStateFor(source).bytes : null;
      if (body !== null && (method === "GET" || method === "HEAD"))
        throw new TypeError(`a ${method} request cannot have a body`);
      const signal = options.signal ?? source?.signal ?? null;
      if (signal !== null && !(signal instanceof AbortSignal))
        throw new TypeError("fetch signal must be an AbortSignal");
      Object.defineProperties(this, {
        method: { value: method, enumerable: true },
        url: { value: resolveAgainstDocument(source ? source.url : String(input)).href, enumerable: true },
        headers: { value: headers, enumerable: true },
        signal: { value: signal, enumerable: true },
      });
      bodyStates.set(this, { used: false, id: null, bytes: body });
    }
  }
  installBodyMethods(Request.prototype);

  class Response {
    constructor(body = null, options = {}) {
      const status = options.status === undefined ? 200 : Number(options.status);
      if (!Number.isInteger(status) || status < 200 || status > 599)
        throw new RangeError(`invalid response status: ${options.status}`);
      const headers = new Headers(options.headers);
      Object.defineProperties(this, {
        status: { value: status, enumerable: true },
        statusText: { value: String(options.statusText ?? ""), enumerable: true },
        headers: { value: headers, enumerable: true },
        ok: { value: status >= 200 && status < 300, enumerable: true },
        url: { value: "", enumerable: true },
        redirected: { value: false, enumerable: true },
      });
      bodyStates.set(this, { used: false, id: null, bytes: encodeBody(body, headers) });
    }
    static json(data, options = {}) {
      const response = new Response(JSON.stringify(data), options);
      response.headers.set("content-type", "application/json");
      return response;
    }
  }
  installBodyMethods(Response.prototype);

  const receivedResponse = record => {
    const response = Object.create(Response.prototype);
    Object.defineProperties(response, {
      status: { value: record.status, enumerable: true },
      statusText: { value: record.statusText, enumerable: true },
      headers: { value: new Headers(record.headers), enumerable: true },
      ok: { value: record.ok, enumerable: true },
      url: { value: record.url, enumerable: true },
      redirected: { value: record.redirected, enumerable: true },
    });
    bodyStates.set(response, { used: false, id: record.id, bytes: null });
    abandonedBodies.register(response, record.id, response);
    return response;
  };

  const inflightFetches = new Map();
  const fetchFailure = error => error.name === "TypeError"
    ? new TypeError(error.message)
    : new DOMException(error.message, error.name);
  // The one handoff point for network work: completions become settled promises
  // here, before any requestAnimationFrame callback of the same turn runs.
  const settleFetches = () => {
    if (inflightFetches.size === 0) return;
    for (const record of JSON.parse(__blitsenFetchPoll()).completed) {
      const pending = inflightFetches.get(record.id);
      if (!pending) { __blitsenFetchCancel(String(record.id)); continue; }
      inflightFetches.delete(record.id);
      pending.detach();
      if (record.error) pending.reject(fetchFailure(record.error));
      else pending.resolve(receivedResponse(record));
    }
  };

  const fetch = (input, options = {}) => {
    let request;
    let id;
    try {
      request = new Request(input, options);
      if (request.signal?.aborted) return Promise.reject(signalState(request.signal).reason);
      const state = bodyStateFor(request);
      state.used = true;
      id = __blitsenFetchStart(JSON.stringify({
        url: request.url, method: request.method, headers: [...request.headers],
      }), state.bytes);
    } catch (error) {
      return Promise.reject(error);
    }
    return new Promise((resolve, reject) => {
      const signal = request.signal;
      const onAbort = signal && (() => {
        inflightFetches.delete(id);
        __blitsenFetchCancel(String(id));
        reject(signalState(signal).reason);
      });
      inflightFetches.set(id, {
        resolve, reject,
        detach: () => { if (onAbort) signal.removeEventListener("abort", onAbort); },
      });
      if (onAbort) signal.addEventListener("abort", onAbort, { once: true });
    });
  };

  // `window.stop()`: abort the document's in-flight loading. Every outstanding
  // `fetch` is rejected the way its own AbortSignal would reject it, and every
  // subresource the renderer is still waiting on is cancelled and settled — a
  // request left pending would block painting rather than end the load.
  //
  // Timers and animation frames are left running, because a browser leaves them
  // running: they are the application's own work, not the document's load. Nor
  // is there a parser to stop; a Blitsen document is parsed whole before any
  // script of it runs. With nothing in flight both halves still run and find
  // nothing, which is a no-op in effect rather than a no-op implementation.
  const stop = () => {
    for (const [id, pending] of inflightFetches) {
      inflightFetches.delete(id);
      pending.detach();
      __blitsenFetchCancel(String(id));
      pending.reject(new DOMException("The operation was aborted", "AbortError"));
    }
    call("stopLoading");
  };

  // Location and history. In-memory only: no navigation, no network, no
  // back-forward cache. The address is synthetic because an exported
  // application has no server and therefore no origin; it is path-rooted
  // because that is what a client-side router reads.
  const documentUrl = call("documentUrl");
  let historyEntries = [{ url: documentUrl, state: null }];
  let historyIndex = 0;
  let scrollRestoration = "auto";
  let locationParts = call("urlParts", documentUrl);
  const currentUrl = () => historyEntries[historyIndex].url;
  const refreshLocation = () => { locationParts = call("urlParts", currentUrl()); };
  const resolveAgainstDocument = url => call("resolveUrl", currentUrl(), String(url));
  const sameDocumentTarget = url => {
    const target = resolveAgainstDocument(url);
    if (!target.sameOrigin)
      throw new DOMException(`cannot reach ${target.href} from ${currentUrl()}`, "SecurityError");
    return target.href;
  };
  const pushEntry = (url, state) => {
    historyEntries.length = historyIndex + 1;
    historyEntries.push({ url, state });
    historyIndex = historyEntries.length - 1;
    refreshLocation();
  };

  class PopStateEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperty(this, "state", { value: options.state ?? null, enumerable: true });
    }
  }

  class HashChangeEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperties(this, {
        oldURL: { value: String(options.oldURL ?? ""), enumerable: true },
        newURL: { value: String(options.newURL ?? ""), enumerable: true },
      });
    }
  }

  const traverseHistory = delta => {
    const next = Math.min(historyEntries.length - 1, Math.max(0, historyIndex + delta));
    if (next === historyIndex) return;
    const previous = locationParts;
    historyIndex = next;
    refreshLocation();
    globalThis.dispatchEvent(new PopStateEvent("popstate", { state: historyEntries[historyIndex].state }));
    if (previous.hash !== locationParts.hash)
      globalThis.dispatchEvent(new HashChangeEvent("hashchange",
        { oldURL: previous.href, newURL: locationParts.href }));
  };

  class History {
    constructor() { throw new TypeError("Illegal constructor"); }
    get length() { return historyEntries.length; }
    get state() { return historyEntries[historyIndex].state; }
    get scrollRestoration() { return scrollRestoration; }
    set scrollRestoration(value) { if (value === "auto" || value === "manual") scrollRestoration = value; }
    pushState(state, unused, url) {
      pushEntry(url == null ? currentUrl() : sameDocumentTarget(url), state ?? null);
    }
    replaceState(state, unused, url) {
      historyEntries[historyIndex] = { url: url == null ? currentUrl() : sameDocumentTarget(url), state: state ?? null };
      refreshLocation();
    }
    // Traversal is a task on the web, and routers rely on observing popstate
    // after their own call returns.
    go(delta = 0) { setTimeout(() => traverseHistory(Math.trunc(Number(delta)) || 0), 0); }
    back() { this.go(-1); }
    forward() { this.go(1); }
  }

  const noDocumentNavigation = property => {
    throw new DOMException(
      `Blitsen has no document navigation; use history.pushState instead of assigning location.${property}`,
      "NotSupportedError");
  };

  class Location {
    constructor() { throw new TypeError("Illegal constructor"); }
    get href() { return locationParts.href; }
    set href(value) { noDocumentNavigation("href"); }
    get protocol() { return locationParts.protocol; }
    get host() { return locationParts.host; }
    get hostname() { return locationParts.hostname; }
    get port() { return locationParts.port; }
    get origin() { return locationParts.origin; }
    get pathname() { return locationParts.pathname; }
    set pathname(value) { noDocumentNavigation("pathname"); }
    get search() { return locationParts.search; }
    set search(value) { noDocumentNavigation("search"); }
    get hash() { return locationParts.hash; }
    set hash(value) {
      const text = String(value);
      const target = sameDocumentTarget(text.startsWith("#") ? text : `#${text}`);
      if (target === currentUrl()) return;
      const previous = locationParts;
      pushEntry(target, null);
      globalThis.dispatchEvent(new HashChangeEvent("hashchange",
        { oldURL: previous.href, newURL: locationParts.href }));
    }
    toString() { return locationParts.href; }
  }

  const location = Object.create(Location.prototype);
  const history = Object.create(History.prototype);

  // Storage. In memory for the life of the process, and no more than that:
  // there is no profile directory behind an exported application yet, so
  // `localStorage` here keeps a session rather than a preference. The reason it
  // exists at all is that its absence is not survivable — libraries read it
  // unguarded inside a render — while its forgetfulness is, and `doctor` reports
  // that forgetfulness on every build rather than leaving it to be discovered.
  const storageEntries = new WeakMap();
  const entriesOf = storage => {
    const entries = storageEntries.get(storage);
    if (!entries) throw new TypeError("Illegal invocation");
    return entries;
  };

  class Storage {
    constructor() { storageEntries.set(this, new Map()); }
    get length() { return entriesOf(this).size; }
    key(index) { return [...entriesOf(this).keys()][Number(index)] ?? null; }
    getItem(key) { return entriesOf(this).get(String(key)) ?? null; }
    setItem(key, value) { entriesOf(this).set(String(key), String(value)); }
    removeItem(key) { entriesOf(this).delete(String(key)); }
    clear() { entriesOf(this).clear(); }
  }

  // Property access is the same store as `getItem`, so `storage.theme = "dark"`
  // cannot diverge from `storage.setItem("theme", "dark")`.
  const storageArea = () => {
    const storage = new Storage();
    const area = new Proxy(storage, {
      get(target, key, receiver) {
        return typeof key !== "string" || key in target
          ? Reflect.get(target, key, receiver) : target.getItem(key) ?? undefined;
      },
      set(target, key, value, receiver) {
        if (typeof key !== "string" || key in target) return Reflect.set(target, key, value, receiver);
        target.setItem(key, value);
        return true;
      },
      has(target, key) {
        return key in target || (typeof key === "string" && target.getItem(key) !== null);
      },
      deleteProperty(target, key) { target.removeItem(key); return true; },
      ownKeys(target) { return [...entriesOf(target).keys()]; },
      getOwnPropertyDescriptor(target, key) {
        const value = typeof key === "string" ? target.getItem(key) : null;
        return value === null ? undefined : { value, writable: true, enumerable: true, configurable: true };
      },
    });
    // A method reached through the proxy is called with the proxy as `this`, so
    // both objects have to find the same entries.
    storageEntries.set(area, entriesOf(storage));
    return area;
  };
  const localStorage = storageArea();
  const sessionStorage = storageArea();

  // Identity, never capability. These three are facts about the machine the
  // application is running on, which is why they can be answered at all; every
  // capability `navigator` normally carries stays absent so that feature
  // detection still selects a fallback path.
  const navigatorFacts = JSON.parse(__blitsenNavigatorState);

  class Navigator {
    constructor() { throw new TypeError("Illegal constructor"); }
    get userAgent() { return navigatorFacts.userAgent; }
    get platform() { return navigatorFacts.platform; }
    get language() { return navigatorFacts.language; }
    get languages() { return Object.freeze([navigatorFacts.language]); }
  }

  const navigator = Object.create(Navigator.prototype);

  for (const method of ["addEventListener", "removeEventListener", "dispatchEvent"])
    Object.defineProperty(globalThis, method, { value: EventTarget.prototype[method], configurable: true });
  const globals = {
    EventTarget, Node, Element, NodeList, Document, DocumentFragment, DOMTokenList,
    Attr, NamedNodeMap,
    CSSStyleDeclaration, MutationObserver, ResizeObserver, HTMLElement, HTMLIFrameElement,
    SVGElement, Text, Comment, Image,
    HTMLImageElement, HTMLLinkElement, HTMLTemplateElement, Storage, Navigator, document,
    HTMLInputElement, HTMLTextAreaElement, HTMLSelectElement, HTMLOptionElement,
    HTMLButtonElement, HTMLFormElement,
    BlitsenViewElement, BlitsenViewSurface,
    getComputedStyle, matchMedia, MediaQueryList, MediaQueryListEvent,
    Event, MouseEvent, KeyboardEvent, CustomEvent, SubmitEvent, PopStateEvent, HashChangeEvent,
    Headers, Request, Response, Blob, AbortController, AbortSignal, fetch, stop,
    Location, History,
    requestAnimationFrame, cancelAnimationFrame,
    setTimeout, clearTimeout, setInterval, clearInterval,
    __blitsenAnimationFrameTick: animationFrameTick,
    __blitsenAnimationFramesPending: () =>
      animationFrames.size > 0 || inflightFetches.size > 0 || pendingResizeObservations() > 0
      || waitingImages() > 0,
    __blitsenForcedLayoutsThisFrame: () => forcedLayoutsThisFrame,
    __blitsenEventInternals: eventInternals,
    __blitsenDispatchMouseEvent: dispatchMouseEvent,
    __blitsenDispatchKeyboardEvent: dispatchKeyboardEvent,
    __blitsenDispatchLifecycleEvent: dispatchLifecycleEvent,
    __blitsenDisposeContext: () => {
      for (const id of contextTimeouts) hostClearTimeout(id);
      for (const id of contextIntervals) hostClearInterval(id);
      contextTimeouts.clear();
      contextIntervals.clear();
      animationFrames.clear();
      runningAnimationFrames?.clear();
      acquiredSurfaces.clear();
      resizeObservers.clear();
      mediaQueryLists.clear();
      wrapperCache.clear();
      pendingImages.clear();
      inflightFetches.clear();
      __blitsenFetchDispose();
      historyEntries = [{ url: documentUrl, state: null }];
      historyIndex = 0;
      refreshLocation();
      Object.assign(globalThis, {
        setTimeout: hostSetTimeout, clearTimeout: hostClearTimeout,
        setInterval: hostSetInterval, clearInterval: hostClearInterval,
      });
    },
    // `WindowState::install` clears the host's browser globals after this
    // script runs, so the ones Blitsen replaces rather than deletes are attached
    // from there rather than here.
    __blitsenInstallReplacedGlobals: () => {
      delete globalThis.__blitsenInstallReplacedGlobals;
      for (const [name, value] of [["location", location], ["history", history],
        ["navigator", navigator], ["localStorage", localStorage], ["sessionStorage", sessionStorage]])
        Object.defineProperty(globalThis, name, { value, enumerable: true, configurable: true });
    },
  };
  if (testHarness) globals.__blitsenInjectMouseEvent = (type, target, init = {}) => {
    if (!(target instanceof Node)) throw new TypeError("mouse event target must be a Node");
    return dispatchMouseEvent(String(type), target[handle], init);
  };
  // Injects at viewport coordinates the way the native window does: hit test the
  // laid-out tree first, then dispatch at whatever that resolves to. Harness-only,
  // so tests exercise the same path as real input rather than picking a target.
  if (testHarness) globals.__blitsenInjectPointerAt = (type, clientX, clientY, init = {}) => {
    const hit = call("hitTest", Number(clientX), Number(clientY));
    if (!hit) return null;
    const allowed = dispatchMouseEvent(String(type), hit.target, {
      bubbles: true, cancelable: true,
      clientX: Number(clientX), clientY: Number(clientY),
      screenX: Number(clientX), screenY: Number(clientY),
      offsetX: hit.offsetX, offsetY: hit.offsetY,
      button: 0, buttons: type === "mousedown" ? 1 : 0,
      ...init,
    });
    return { allowed, target: wrap(hit.target), path: hit.path.map(wrap) };
  };
  Object.assign(globalThis, globals);
  globalThis.window = globalThis;
  // Absent, not stubbed: an unimplemented API must not exist, so feature
  // detection selects a fallback. The Phase 1 host supplies several of these
  // itself, and leaving those in place would make them disappear at the Phase 2
  // engine swap. `packages/blitsen/src/api-manifest.mjs` reads this list, and
  // refuses to generate a manifest that describes any other API as absent.
  for (const key of ["requestIdleCallback", "cancelIdleCallback", "indexedDB",
    "Worker", "SharedWorker", "ServiceWorker", "ServiceWorkerContainer",
    "MessageChannel", "MessagePort", "BroadcastChannel", "postMessage",
    "WebSocket", "EventSource", "XMLHttpRequest",
    "ReadableStream", "WritableStream", "TransformStream",
    "FormData", "File", "FileReader",
    "HTMLCanvasElement", "CanvasRenderingContext2D", "OffscreenCanvas", "ImageData", "Path2D",
    "WebGLRenderingContext", "WebGL2RenderingContext", "GPUCanvasContext",
    "Audio", "AudioContext", "webkitAudioContext", "HTMLMediaElement",
    "alert", "confirm", "prompt", "print",
    "open", "close", "navigation",
    "cookieStore", "screen", "Notification", "caches",
    "IntersectionObserver", "PerformanceObserver",
    "CSSStyleSheet", "StyleSheetList",
    "customElements", "ShadowRoot", "DOMParser"]) {
    try { delete globalThis[key]; } catch {}
  }
})();
"##;

/// Installs the real DOM object graph into a Node-API JavaScript environment.
pub(super) fn install(
    engine: &mut NodeApiEngine,
    runtime: DomRuntime,
    width: u32,
    height: u32,
    device_pixel_ratio: f64,
    test_harness: bool,
) -> Result<Rc<RefCell<WindowState>>, JsError> {
    let class = Rc::new(engine.register_class(NativeClass::new("BlitsenNode"))?);
    let table = Rc::new(WrapperTable::<NodeId, NodeWeakRef>::new());
    let raw_env = engine.raw_env();

    let wrapper_runtime = runtime.clone();
    let wrapper_table = Rc::clone(&table);
    let wrapper_class = Rc::clone(&class);
    let wrap_function = engine.define_function(
        "__blitsenWrap",
        Box::new(move |call| {
            let handle = argument(&call.arguments, 0, "node handle")?;
            let node = wrapper_runtime.resolve_handle(&handle)?;
            let mut callback_engine = NodeApiEngine::new(Env::from_raw(raw_env));
            wrapper_table.get_or_create(&mut callback_engine, node, |engine, table_finalizer| {
                wrapper_runtime.retain_handle(&handle)?;
                let finalizer_runtime = wrapper_runtime.clone();
                let finalizer_handle = handle.clone();
                let finalizer = Box::new(move |external| {
                    table_finalizer(external);
                    let _ = finalizer_runtime.release_handle(&finalizer_handle);
                });
                match engine.instantiate(&wrapper_class, ExternalId(node.as_u64()), Some(finalizer))
                {
                    Ok(wrapper) => Ok(wrapper),
                    Err(error) => {
                        let _ = wrapper_runtime.release_handle(&handle);
                        Err(error)
                    }
                }
            })
        }),
    )?;
    engine.set_global("__blitsenWrap", &wrap_function)?;

    let dispatch_runtime = runtime.clone();
    let call_function = engine.define_function(
        "__blitsenDomCall",
        Box::new(move |call| {
            let operation = argument(&call.arguments, 0, "operation")?;
            let arguments = call
                .arguments
                .iter()
                .skip(1)
                .map(callback_string)
                .collect::<Result<Vec<_>, _>>()?;
            let result = dispatch(&dispatch_runtime, &operation, &arguments)?;
            json_string(raw_env, &result)
        }),
    )?;
    engine.set_global("__blitsenDomCall", &call_function)?;
    let default_scroll_runtime = runtime.clone();
    let default_scroll_function = engine.define_function(
        "__blitsenScrollDefault",
        Box::new(move |call| {
            let handle = argument(&call.arguments, 0, "scroll target")?;
            let delta_x = argument(&call.arguments, 1, "horizontal scroll delta")?
                .parse::<f64>()
                .map_err(|_| JsError::new("invalid horizontal scroll delta"))?;
            let delta_y = argument(&call.arguments, 2, "vertical scroll delta")?
                .parse::<f64>()
                .map_err(|_| JsError::new("invalid vertical scroll delta"))?;
            let node = default_scroll_runtime.resolve_handle(&handle)?;
            let mut document = default_scroll_runtime.document.borrow_mut();
            document
                .flush_layout()
                .map_err(|error| JsError::new(error.to_string()))?;
            document
                .document_mut()
                .scroll_node_by(node, delta_x, delta_y, |_| {});
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenScrollDefault", &default_scroll_function)?;
    let viewport_runtime = runtime.clone();
    let viewport_write_function = engine.define_function(
        "__blitsenViewportWrite",
        Box::new(move |call| {
            let handle = argument(&call.arguments, 0, "viewport handle")?;
            let node = viewport_runtime.resolve_handle(&handle)?;
            let pixels = call
                .arguments
                .get(1)
                .ok_or_else(|| JsError::new("viewport surface contents are required"))?;
            let mut callback_engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let pixels = callback_engine.to_typed_array(pixels)?;
            if !matches!(
                pixels.kind,
                TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped
            ) {
                return Err(JsError::new(
                    "viewport surface contents must be a Uint8Array or Uint8ClampedArray",
                ));
            }
            viewport_runtime
                .document
                .borrow_mut()
                .write_native_viewport(node, &pixels.bytes)
                .map_err(|error| JsError::new(error.to_string()))?;
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenViewportWrite", &viewport_write_function)?;
    install_text_codec(engine, raw_env)?;
    install_fetch(engine, raw_env)?;
    let dev_layout_warnings = std::env::var("BLITSEN_DEV_LAYOUT_WARNINGS").is_ok_and(|value| {
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    });
    let dev_layout_warnings = engine.boolean(dev_layout_warnings);
    engine.set_global("__blitsenDevLayoutWarnings", &dev_layout_warnings)?;
    let navigator = json_string(raw_env, &navigator_state())?;
    engine.set_global("__blitsenNavigatorState", &navigator)?;
    let test_harness = engine.boolean(test_harness);
    engine.set_global("__blitsenTestHarness", &test_harness)?;
    engine.evaluate_script(BOOTSTRAP, "blitsen:dom-bootstrap")?;

    let document = engine.evaluate_script("globalThis.document", "blitsen:document-value")?;
    let window_state = Rc::new(RefCell::new(WindowState::new(
        width,
        height,
        device_pixel_ratio,
    )));
    window_state.borrow().install(engine, &document)?;
    engine.evaluate_script(
        "globalThis.__blitsenInstallReplacedGlobals()",
        "blitsen:install-replaced-globals",
    )?;
    let resize_state = Rc::clone(&window_state);
    let resize_runtime = runtime;
    let resize_function = engine.define_function(
        "__blitsenWindowResize",
        Box::new(move |call| {
            let width = argument(&call.arguments, 0, "viewport width")?
                .parse::<u32>()
                .map_err(|_| JsError::new("invalid viewport width"))?;
            let height = argument(&call.arguments, 1, "viewport height")?
                .parse::<u32>()
                .map_err(|_| JsError::new("invalid viewport height"))?;
            resize_state.borrow_mut().resize(width, height);
            let mut document = resize_runtime.document.borrow_mut();
            let mut viewport = document.document_ref().viewport().clone();
            viewport.window_size = (width, height);
            document.document_mut().set_viewport(viewport);
            drop(document);
            let mut callback_engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let window =
                callback_engine.evaluate_script("globalThis", "blitsen:window-resize-target")?;
            resize_state.borrow().sync(&mut callback_engine, &window)?;
            callback_engine.evaluate_script(
                "globalThis.__blitsenDispatchLifecycleEvent('resize')",
                "blitsen:test-window-resize",
            )?;
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenWindowResize", &resize_function)?;
    Ok(window_state)
}

/// The three facts `navigator` is allowed to state about this machine.
///
/// Identity, never capability: see COMPATIBILITY.md for why the rest of the
/// interface stays absent. The user-agent string names Blitsen rather than
/// impersonating a browser, because an application that sniffs it deserves a
/// true answer more than it deserves a code path written for someone else.
fn navigator_state() -> Value {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", _) => "MacIntel".to_owned(),
        ("windows", _) => "Win32".to_owned(),
        (os, arch) => format!("{}{} {arch}", os[..1].to_uppercase(), &os[1..]),
    };
    // POSIX locales are `en_GB.UTF-8`; BCP 47 is `en-GB`.
    let language = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .map(|locale| {
            locale
                .split(['.', '@'])
                .next()
                .unwrap_or_default()
                .replace('_', "-")
        })
        .filter(|locale| !locale.is_empty() && locale != "C" && locale != "POSIX")
        .unwrap_or_else(|| "en-US".to_owned());
    json!({
        "userAgent": format!("Blitsen/{} ({platform})", env!("CARGO_PKG_VERSION")),
        "platform": platform,
        "language": language,
    })
}

/// Installs the UTF-8 conversions the body classes need.
///
/// `TextEncoder` and `TextDecoder` are Web IDL, not ECMAScript: relying on the
/// host's would make the request and response bodies change shape under the
/// Phase 2 engine.
fn install_text_codec(engine: &mut NodeApiEngine, raw_env: sys::napi_env) -> Result<(), JsError> {
    let encode = engine.define_function(
        "__blitsenUtf8Encode",
        Box::new(move |call| {
            let text = argument(&call.arguments, 0, "text")?;
            let bytes = TypedArray::new(TypedArrayKind::Uint8, text.into_bytes())?;
            NodeApiEngine::new(Env::from_raw(raw_env)).typed_array(&bytes)
        }),
    )?;
    engine.set_global("__blitsenUtf8Encode", &encode)?;
    let decode = engine.define_function(
        "__blitsenUtf8Decode",
        Box::new(move |call| {
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let bytes = call
                .arguments
                .first()
                .ok_or_else(|| JsError::new("missing bytes"))
                .and_then(|value| engine.to_typed_array(value))?;
            engine.string(&String::from_utf8_lossy(&bytes.bytes))
        }),
    )?;
    engine.set_global("__blitsenUtf8Decode", &decode)
}

/// Installs the transport the bootstrap's `fetch` classes call through.
fn install_fetch(engine: &mut NodeApiEngine, raw_env: sys::napi_env) -> Result<(), JsError> {
    let host = Rc::new(fetch::FetchHost::new()?);

    let start_host = Rc::clone(&host);
    let start = engine.define_function(
        "__blitsenFetchStart",
        Box::new(move |call| {
            let spec = argument(&call.arguments, 0, "fetch request")?;
            let spec = serde_json::from_str(&spec)
                .map_err(|error| JsError::new(format!("invalid fetch request: {error}")))?;
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let body = match call.arguments.get(1) {
                Some(value) if engine.value_type(value)? == JsType::TypedArray => {
                    Some(engine.to_typed_array(value)?.bytes)
                }
                _ => None,
            };
            let id = start_host.start(&spec, body)?;
            Ok(engine.number(id as f64))
        }),
    )?;
    engine.set_global("__blitsenFetchStart", &start)?;

    let poll_host = Rc::clone(&host);
    let poll = engine.define_function(
        "__blitsenFetchPoll",
        Box::new(move |_| json_string(raw_env, &poll_host.poll())),
    )?;
    engine.set_global("__blitsenFetchPoll", &poll)?;

    let body_host = Rc::clone(&host);
    let body = engine.define_function(
        "__blitsenFetchBody",
        Box::new(move |call| {
            let id = fetch_id(&call.arguments)?;
            let kind = argument(&call.arguments, 1, "body kind")?;
            let bytes = body_host.take_body(id)?;
            let mut engine = NodeApiEngine::new(Env::from_raw(raw_env));
            match kind.as_str() {
                "text" => engine.string(&String::from_utf8_lossy(&bytes)),
                "bytes" => engine.typed_array(&TypedArray::new(TypedArrayKind::Uint8, bytes)?),
                other => Err(JsError::new(format!("invalid body kind: {other}"))),
            }
        }),
    )?;
    engine.set_global("__blitsenFetchBody", &body)?;

    let cancel_host = Rc::clone(&host);
    let cancel = engine.define_function(
        "__blitsenFetchCancel",
        Box::new(move |call| {
            cancel_host.cancel(fetch_id(&call.arguments)?);
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenFetchCancel", &cancel)?;

    let dispose = engine.define_function(
        "__blitsenFetchDispose",
        Box::new(move |call| {
            host.dispose();
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenFetchDispose", &dispose)
}

fn fetch_id(arguments: &[Unknown<'static>]) -> Result<u64, JsError> {
    argument(arguments, 0, "request id")?
        .parse::<u64>()
        .map_err(|_| JsError::new("invalid fetch request id"))
}

fn argument(arguments: &[Unknown<'static>], index: usize, name: &str) -> Result<String, JsError> {
    arguments
        .get(index)
        .ok_or_else(|| JsError::new(format!("missing {name}")))
        .and_then(callback_string)
}

fn bridge_arg<'a>(arguments: &'a [String], index: usize, name: &str) -> Result<&'a str, JsError> {
    arguments
        .get(index)
        .map(String::as_str)
        .ok_or_else(|| JsError::new(format!("missing {name}")))
}

fn handle(_runtime: &DomRuntime, arguments: &[String], index: usize) -> Result<NodeId, JsError> {
    bridge_arg(arguments, index, "node handle")?
        .parse::<u64>()
        .map(NodeId::from_u64)
        .map_err(|_| JsError::new("invalid DOM node handle"))
}

fn serialized(node: Option<NodeId>) -> Value {
    node.map(DomRuntime::serialize_handle)
        .map(Value::String)
        .unwrap_or(Value::Null)
}

fn dom_error(error: DomError) -> JsError {
    JsError::new(error.to_string())
}

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const MATHML_NAMESPACE: &str = "http://www.w3.org/1998/Math/MathML";

/// The element a fragment's children are parked under.
///
/// A `DocumentFragment` is a real detached element in the backend: that is what
/// gives its children a parent to be parsed, serialized and cloned against, and
/// it is never connected, so it is never styled, laid out or painted. The name
/// is `template` because template contents are the one parsing context that
/// accepts every kind of child, including the table rows an ordinary element
/// would discard.
const FRAGMENT_TAG: &str = "template";

fn namespace_uri(namespace: &Namespace) -> Option<&str> {
    match namespace {
        Namespace::Html => Some(HTML_NAMESPACE),
        Namespace::Svg => Some(SVG_NAMESPACE),
        Namespace::MathMl => Some(MATHML_NAMESPACE),
        Namespace::None => None,
        Namespace::Other(uri) => Some(uri),
    }
}

fn namespace_from_uri(uri: &str) -> Namespace {
    match uri {
        "" => Namespace::None,
        HTML_NAMESPACE => Namespace::Html,
        SVG_NAMESPACE => Namespace::Svg,
        MATHML_NAMESPACE => Namespace::MathMl,
        other => Namespace::Other(other.to_owned()),
    }
}

fn element_name(namespace: &str, name: &str) -> Result<DomName, JsError> {
    if name.is_empty()
        || name.chars().any(|character| {
            character.is_whitespace() || matches!(character, '<' | '>' | '/' | '\0')
        })
    {
        return Err(JsError::new("invalid element name"));
    }
    let namespace = namespace_from_uri(namespace);
    // Only HTML folds case; SVG has `linearGradient` and `clipPath`.
    let local = if namespace == Namespace::Html {
        name.to_ascii_lowercase()
    } else {
        name.to_owned()
    };
    Ok(DomName { namespace, local })
}

/// Builds the attribute name behind the `*AttributeNS` trio.
///
/// A qualified name's prefix is not kept: Blitz keys an attribute by namespace
/// and local name, which is the pair `getAttributeNS` asks back for. Case folds
/// only in the null namespace — the one ordinary HTML attributes live in, which
/// the rest of the bridge already lower-cases — so `xlink:href` and `xml:space`
/// keep theirs, as `createElementNS` keeps an element's.
fn attribute_name(namespace: &str, qualified: &str) -> Result<DomName, JsError> {
    let local = qualified.rsplit(':').next().unwrap_or_default();
    if local.is_empty()
        || local.chars().any(|character| {
            character.is_whitespace() || matches!(character, '<' | '>' | '/' | '=' | '"' | '\0')
        })
    {
        return Err(JsError::new("invalid attribute name"));
    }
    let namespace = namespace_from_uri(namespace);
    let local = if namespace == Namespace::None {
        local.to_ascii_lowercase()
    } else {
        local.to_owned()
    };
    Ok(DomName { namespace, local })
}

/// Returns the descendants of `root` carrying every one of `names` as a class.
///
/// Matched against the class attribute's tokens rather than through a selector:
/// a class a bundler invents contains characters (`w-1/2`, `md:flex`) that only
/// survive a selector escaped, and the escaping is what would be guessed at.
fn elements_by_class_name(
    dom: &BlitzDom,
    root: NodeId,
    names: &str,
) -> Result<Vec<NodeId>, JsError> {
    let tokens = names.split_ascii_whitespace().collect::<Vec<_>>();
    let mut found = Vec::new();
    if tokens.is_empty() {
        return Ok(found);
    }
    let mut pending = dom.children(root).map_err(dom_error)?;
    pending.reverse();
    while let Some(node) = pending.pop() {
        if dom.node_kind(node).map_err(dom_error)? != NodeKind::Element {
            continue;
        }
        let classes = dom
            .attribute(node, &DomName::attribute("class"))
            .map_err(dom_error)?
            .unwrap_or_default();
        if tokens
            .iter()
            .all(|token| classes.split_ascii_whitespace().any(|name| name == *token))
        {
            found.push(node);
        }
        let mut children = dom.children(node).map_err(dom_error)?;
        children.reverse();
        pending.extend(children);
    }
    Ok(found)
}

/// Returns an element's attribute names in document order.
///
/// Read through the renderer's own view of the node: the DOM boundary can read
/// one attribute by name but cannot enumerate them, and `dataset` has to know
/// which `data-` attributes exist before it can answer for them. Namespaced,
/// because a clone reads its attributes back through this and `xlink:href`
/// copied into the null namespace would be a different attribute.
fn attribute_names(dom: &BlitzDom, node: NodeId) -> Result<Vec<DomName>, JsError> {
    Ok(dom
        .document_ref()
        .get_node(node)
        .ok_or_else(|| dom_error(DomError::StaleNode))?
        .element_data()
        .ok_or_else(|| dom_error(DomError::InvalidNodeType))?
        .attrs()
        .iter()
        .map(|attribute| DomName {
            namespace: namespace_from_uri(&attribute.name.ns),
            local: attribute.name.local.to_string(),
        })
        .collect())
}

/// Copies a node, deeply when asked, the way `cloneNode` defines it.
///
/// A clone carries the tree and nothing else: no listeners, no wrapper identity
/// and no JavaScript state, which is what the DOM specifies. Depth is served by
/// serializing and reparsing, because that is the only complete copy the DOM
/// boundary offers.
fn clone_node(dom: &mut BlitzDom, node: NodeId, deep: bool) -> Result<NodeId, JsError> {
    match dom.node_kind(node).map_err(dom_error)? {
        NodeKind::Element => {
            let name = dom.element_name(node).map_err(dom_error)?;
            let clone = dom.create_element(&name).map_err(dom_error)?;
            for attribute in attribute_names(dom, node)? {
                if let Some(value) = dom.attribute(node, &attribute).map_err(dom_error)? {
                    dom.set_attribute(clone, &attribute, &value)
                        .map_err(dom_error)?;
                }
            }
            if deep {
                let html = dom.inner_html(node).map_err(dom_error)?;
                dom.set_inner_html(clone, &html).map_err(dom_error)?;
            }
            Ok(clone)
        }
        NodeKind::Text => {
            let text = dom.text_content(node).map_err(dom_error)?;
            dom.create_text(&text).map_err(dom_error)
        }
        NodeKind::Comment => {
            let data = comment_data(dom, node)?;
            create_comment(dom, &data)
        }
        NodeKind::Document | NodeKind::Fragment => Err(dom_error(DomError::InvalidNodeType)),
    }
}

fn comment_data(dom: &BlitzDom, node: NodeId) -> Result<String, JsError> {
    match &dom
        .document_ref()
        .get_node(node)
        .ok_or_else(|| dom_error(DomError::StaleNode))?
        .data
    {
        blitz::dom::NodeData::Comment { contents } => Ok(contents.clone()),
        _ => Err(dom_error(DomError::InvalidNodeType)),
    }
}

/// Creates a detached comment node by parsing one.
///
/// The DOM boundary has no comment constructor, so the fragment parser is the
/// way to reach the node kind. Data that would close the comment early is
/// refused rather than silently truncated.
fn create_comment(dom: &mut BlitzDom, data: &str) -> Result<NodeId, JsError> {
    if data.contains("-->")
        || data.contains("--!>")
        || data.starts_with('>')
        || data.starts_with("->")
    {
        return Err(JsError::new(
            "comment data cannot contain a comment terminator",
        ));
    }
    let context = dom
        .body()
        .or_else(|| dom.document_element())
        .ok_or_else(|| dom_error(DomError::NotFound))?;
    let nodes = dom
        .parse_fragment(context, &format!("<!--{data}-->"))
        .map_err(dom_error)?;
    match nodes.first() {
        Some(node) if nodes.len() == 1 && dom.node_kind(*node) == Ok(NodeKind::Comment) => {
            Ok(*node)
        }
        _ => Err(JsError::new("comment data could not be represented")),
    }
}

fn dispatch(runtime: &DomRuntime, operation: &str, arguments: &[String]) -> Result<Value, JsError> {
    let shared = runtime.document();
    let mut dom = shared.borrow_mut();
    match operation {
        "kind" => Ok(Value::String(
            match dom
                .node_kind(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
            {
                NodeKind::Element => "element",
                NodeKind::Document => "document",
                NodeKind::Text => "text",
                NodeKind::Comment => "comment",
                NodeKind::Fragment => "fragment",
            }
            .into(),
        )),
        "tagName" => Ok(Value::String(
            dom.element_name(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .local,
        )),
        "namespaceUri" => Ok(namespace_uri(
            &dom.element_name(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .namespace,
        )
        .map(|uri| Value::String(uri.to_owned()))
        .unwrap_or(Value::Null)),
        "querySelector" => Ok(serialized(
            dom.query_selector(dom.document(), bridge_arg(arguments, 0, "selector")?)
                .map_err(dom_error)?,
        )),
        "querySelectorAll" => Ok(json!(
            dom.query_selector_all(dom.document(), bridge_arg(arguments, 0, "selector")?)
                .map_err(dom_error)?
                .into_iter()
                .map(DomRuntime::serialize_handle)
                .collect::<Vec<_>>()
        )),
        "querySelectorIn" => {
            let node = handle(runtime, arguments, 0)?;
            Ok(serialized(
                dom.query_selector(node, bridge_arg(arguments, 1, "selector")?)
                    .map_err(dom_error)?,
            ))
        }
        "querySelectorAllIn" => {
            let node = handle(runtime, arguments, 0)?;
            Ok(json!(
                dom.query_selector_all(node, bridge_arg(arguments, 1, "selector")?)
                    .map_err(dom_error)?
                    .into_iter()
                    .map(DomRuntime::serialize_handle)
                    .collect::<Vec<_>>()
            ))
        }
        "elementsByClassName" => {
            let root = dom.document();
            Ok(json!(
                elements_by_class_name(&dom, root, bridge_arg(arguments, 0, "class names")?)?
                    .into_iter()
                    .map(DomRuntime::serialize_handle)
                    .collect::<Vec<_>>()
            ))
        }
        "elementsByClassNameIn" => {
            let node = handle(runtime, arguments, 0)?;
            Ok(json!(
                elements_by_class_name(&dom, node, bridge_arg(arguments, 1, "class names")?)?
                    .into_iter()
                    .map(DomRuntime::serialize_handle)
                    .collect::<Vec<_>>()
            ))
        }
        // Selector matching against a single element is the renderer's own, not
        // an emulation over `querySelectorAll`: a detached element has no scope
        // to search, and an ancestor walk would rescan the subtree per level.
        "matches" => {
            let node = handle(runtime, arguments, 0)?;
            dom.node_kind(node).map_err(dom_error)?;
            Ok(Value::Bool(
                dom.document_ref()
                    .matches_selector(node, bridge_arg(arguments, 1, "selector")?)
                    .map_err(|error| dom_error(DomError::Syntax(format!("{error:?}"))))?,
            ))
        }
        "closest" => {
            let node = handle(runtime, arguments, 0)?;
            dom.node_kind(node).map_err(dom_error)?;
            Ok(serialized(
                dom.document_ref()
                    .closest(node, bridge_arg(arguments, 1, "selector")?)
                    .map_err(|error| dom_error(DomError::Syntax(format!("{error:?}"))))?,
            ))
        }
        "contains" => {
            let node = handle(runtime, arguments, 0)?;
            let mut candidate = Some(handle(runtime, arguments, 1)?);
            while let Some(current) = candidate {
                if current == node {
                    return Ok(Value::Bool(true));
                }
                candidate = dom.parent(current).map_err(dom_error)?;
            }
            Ok(Value::Bool(false))
        }
        "getElementById" => Ok(serialized(
            dom.get_element_by_id(bridge_arg(arguments, 0, "id")?)
                .map_err(dom_error)?,
        )),
        "layoutMetrics" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let metrics = dom.layout_metrics(node, snapshot).map_err(dom_error)?;
            Ok(json!({
                "forced": forced,
                "x": metrics.rect.x,
                "y": metrics.rect.y,
                "width": metrics.rect.width,
                "height": metrics.rect.height,
                "contentX": metrics.content_rect.x,
                "contentY": metrics.content_rect.y,
                "contentWidth": metrics.content_rect.width,
                "contentHeight": metrics.content_rect.height,
                "offsetWidth": metrics.offset_width,
                "offsetHeight": metrics.offset_height,
                "clientWidth": metrics.client_width,
                "clientHeight": metrics.client_height,
                "scrollLeft": metrics.scroll_left,
                "scrollTop": metrics.scroll_top,
            }))
        }
        "imageState" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let state = dom.image_state(node, snapshot).map_err(dom_error)?;
            Ok(json!({
                "forced": forced,
                "naturalWidth": state.natural_width,
                "naturalHeight": state.natural_height,
                "complete": state.complete,
                "errored": state.errored,
            }))
        }
        "viewportSurface" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let surface = dom
                .native_viewport_surface(node, snapshot)
                .map_err(dom_error)?;
            Ok(json!({
                "forced": forced,
                "width": surface.width,
                "height": surface.height,
                "devicePixelRatio": surface.device_pixel_ratio,
                "generation": surface.generation,
                "byteLength": surface.byte_length(),
            }))
        }
        "hitTest" => {
            let x = bridge_arg(arguments, 0, "hit-test x")?
                .parse::<f32>()
                .map_err(|_| JsError::new("invalid hit-test x"))?;
            let y = bridge_arg(arguments, 1, "hit-test y")?
                .parse::<f32>()
                .map_err(|_| JsError::new("invalid hit-test y"))?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            Ok(match dom.hit_test(x, y, snapshot).map_err(dom_error)? {
                None => Value::Null,
                Some(hit) => json!({
                    "target": DomRuntime::serialize_handle(hit.target),
                    "path": hit.path.into_iter()
                        .map(DomRuntime::serialize_handle)
                        .collect::<Vec<_>>(),
                    "offsetX": hit.offset_x,
                    "offsetY": hit.offset_y,
                }),
            })
        }
        "setScroll" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let axis = bridge_arg(arguments, 1, "scroll axis")?;
            let value = bridge_arg(arguments, 2, "scroll value")?
                .parse::<f64>()
                .map_err(|_| JsError::new("invalid scroll value"))?;
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            match axis {
                "left" => dom
                    .set_scroll_offset(node, Some(value), None, snapshot)
                    .map_err(dom_error)?,
                "top" => dom
                    .set_scroll_offset(node, None, Some(value), snapshot)
                    .map_err(dom_error)?,
                _ => return Err(JsError::new("invalid scroll axis")),
            }
            Ok(json!({ "forced": forced }))
        }
        "createElement" => Ok(serialized(Some(
            dom.create_element(&element_name(
                HTML_NAMESPACE,
                bridge_arg(arguments, 0, "element name")?,
            )?)
            .map_err(dom_error)?,
        ))),
        "createElementNS" => Ok(serialized(Some(
            dom.create_element(&element_name(
                bridge_arg(arguments, 0, "namespace")?,
                bridge_arg(arguments, 1, "element name")?,
            )?)
            .map_err(dom_error)?,
        ))),
        "createTextNode" => Ok(serialized(Some(
            dom.create_text(bridge_arg(arguments, 0, "text")?)
                .map_err(dom_error)?,
        ))),
        "createComment" => Ok(serialized(Some(create_comment(
            &mut dom,
            bridge_arg(arguments, 0, "comment data")?,
        )?))),
        "createFragment" => Ok(serialized(Some(
            dom.create_element(&DomName::html(FRAGMENT_TAG))
                .map_err(dom_error)?,
        ))),
        "cloneNode" => {
            let node = handle(runtime, arguments, 0)?;
            let deep = bridge_arg(arguments, 1, "clone depth")? == "true";
            Ok(serialized(Some(clone_node(&mut dom, node, deep)?)))
        }
        "body" => Ok(serialized(dom.body())),
        "documentElement" => Ok(serialized(dom.document_element())),
        "appendChild" => {
            let parent = handle(runtime, arguments, 0)?;
            let child = handle(runtime, arguments, 1)?;
            dom.append_child(parent, child).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "insertBefore" => {
            let parent = handle(runtime, arguments, 0)?;
            let child = handle(runtime, arguments, 1)?;
            let reference = if bridge_arg(arguments, 2, "reference")?.is_empty() {
                None
            } else {
                Some(handle(runtime, arguments, 2)?)
            };
            dom.insert_before(parent, child, reference)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "removeChild" => {
            let parent = handle(runtime, arguments, 0)?;
            let child = handle(runtime, arguments, 1)?;
            if dom.parent(child).map_err(dom_error)? != Some(parent) {
                return Err(dom_error(DomError::NotFound));
            }
            dom.remove(child).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "remove" => {
            let node = handle(runtime, arguments, 0)?;
            if dom.parent(node).map_err(dom_error)?.is_some() {
                dom.remove(node).map_err(dom_error)?;
            }
            Ok(Value::Null)
        }
        "replaceWith" => {
            let node = handle(runtime, arguments, 0)?;
            let replacement = handle(runtime, arguments, 1)?;
            dom.replace(node, replacement).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "parentNode" => Ok(serialized(
            dom.parent(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "childNodes" => Ok(json!(
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .into_iter()
                .map(DomRuntime::serialize_handle)
                .collect::<Vec<_>>()
        )),
        "childElements" => {
            let children = dom
                .children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?;
            let mut elements = Vec::new();
            for child in children {
                if dom.node_kind(child).map_err(dom_error)? == NodeKind::Element {
                    elements.push(DomRuntime::serialize_handle(child));
                }
            }
            Ok(json!(elements))
        }
        "firstChild" => Ok(serialized(
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .first()
                .copied(),
        )),
        "lastChild" => Ok(serialized(
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .last()
                .copied(),
        )),
        "nextSibling" => Ok(serialized(
            dom.next_sibling(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "previousSibling" => Ok(serialized(
            dom.previous_sibling(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        // Walked in the backend rather than hop by hop from JavaScript: a text
        // node between two elements is ordinary in rendered markup, and every
        // one skipped would otherwise be a call of its own.
        "nextElementSibling" | "previousElementSibling" => {
            let forward = operation == "nextElementSibling";
            let mut sibling = handle(runtime, arguments, 0)?;
            loop {
                let next = if forward {
                    dom.next_sibling(sibling).map_err(dom_error)?
                } else {
                    dom.previous_sibling(sibling).map_err(dom_error)?
                };
                match next {
                    None => return Ok(Value::Null),
                    Some(node) if dom.node_kind(node).map_err(dom_error)? == NodeKind::Element => {
                        return Ok(serialized(Some(node)));
                    }
                    Some(node) => sibling = node,
                }
            }
        }
        "isConnected" => Ok(Value::Bool(
            dom.is_connected(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        // A comment's data is its `textContent`; the renderer's own text
        // collection skips comments, as it must for an element's.
        "textContent" => {
            let node = handle(runtime, arguments, 0)?;
            Ok(Value::String(
                match dom.node_kind(node).map_err(dom_error)? {
                    NodeKind::Comment => comment_data(&dom, node)?,
                    _ => dom.text_content(node).map_err(dom_error)?,
                },
            ))
        }
        "setTextContent" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_text_content(node, bridge_arg(arguments, 1, "text")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "innerHTML" => Ok(Value::String(
            dom.inner_html(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "outerHTML" => Ok(Value::String(
            dom.outer_html(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "setInnerHTML" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inner_html(node, bridge_arg(arguments, 1, "HTML")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        // Parsed in the element the result lands in, which is what makes a `<td>`
        // survive `beforeend` on a `<tr>` and be discarded anywhere else.
        "insertAdjacentHTML" => {
            let node = handle(runtime, arguments, 0)?;
            let position = bridge_arg(arguments, 1, "position")?.to_ascii_lowercase();
            let sibling = matches!(position.as_str(), "beforebegin" | "afterend");
            let parent = if sibling {
                dom.parent(node)
                    .map_err(dom_error)?
                    .ok_or_else(|| dom_error(DomError::NotFound))?
            } else {
                node
            };
            let reference = match position.as_str() {
                "beforebegin" => Some(node),
                "afterend" => dom.next_sibling(node).map_err(dom_error)?,
                "afterbegin" => dom.children(node).map_err(dom_error)?.first().copied(),
                "beforeend" => None,
                _ => return Err(JsError::new("invalid insertAdjacentHTML position")),
            };
            let parsed = dom
                .parse_fragment(parent, bridge_arg(arguments, 2, "HTML")?)
                .map_err(dom_error)?;
            for child in &parsed {
                dom.insert_before(parent, *child, reference)
                    .map_err(dom_error)?;
            }
            Ok(json!(
                parsed
                    .into_iter()
                    .map(DomRuntime::serialize_handle)
                    .collect::<Vec<_>>()
            ))
        }
        "getAttribute" => Ok(dom
            .attribute(
                handle(runtime, arguments, 0)?,
                &DomName::attribute(
                    bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
                ),
            )
            .map_err(dom_error)?
            .map(Value::String)
            .unwrap_or(Value::Null)),
        "setAttribute" => {
            let node = handle(runtime, arguments, 0)?;
            let name = DomName::attribute(
                bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
            );
            dom.set_attribute(node, &name, bridge_arg(arguments, 2, "attribute value")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "removeAttribute" => {
            let node = handle(runtime, arguments, 0)?;
            let name = DomName::attribute(
                bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
            );
            dom.remove_attribute(node, &name).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "getAttributeNS" => Ok(dom
            .attribute(
                handle(runtime, arguments, 0)?,
                &attribute_name(
                    bridge_arg(arguments, 1, "namespace")?,
                    bridge_arg(arguments, 2, "attribute name")?,
                )?,
            )
            .map_err(dom_error)?
            .map(Value::String)
            .unwrap_or(Value::Null)),
        "setAttributeNS" => {
            let node = handle(runtime, arguments, 0)?;
            let name = attribute_name(
                bridge_arg(arguments, 1, "namespace")?,
                bridge_arg(arguments, 2, "attribute name")?,
            )?;
            dom.set_attribute(node, &name, bridge_arg(arguments, 3, "attribute value")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "removeAttributeNS" => {
            let node = handle(runtime, arguments, 0)?;
            let name = attribute_name(
                bridge_arg(arguments, 1, "namespace")?,
                bridge_arg(arguments, 2, "attribute name")?,
            )?;
            dom.remove_attribute(node, &name).map_err(dom_error)?;
            Ok(Value::Null)
        }
        // Each name with the namespace it is in, which is what an attribute node
        // needs to read its own value back and what `attributeNames` cannot say.
        "attributeEntries" => Ok(json!(
            attribute_names(&dom, handle(runtime, arguments, 0)?)?
                .into_iter()
                .map(|name| json!({
                    "namespace": namespace_uri(&name.namespace),
                    "name": name.local,
                }))
                .collect::<Vec<_>>()
        )),
        // Local names: an attribute is keyed by namespace and local name here,
        // so there is no prefix left to qualify one with.
        "attributeNames" => Ok(json!(
            attribute_names(&dom, handle(runtime, arguments, 0)?)?
                .into_iter()
                .map(|name| name.local)
                .collect::<Vec<_>>()
        )),
        "hasAttribute" => Ok(Value::Bool(
            dom.attribute(
                handle(runtime, arguments, 0)?,
                &DomName::attribute(
                    bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
                ),
            )
            .map_err(dom_error)?
            .is_some(),
        )),
        // Form-control state, which is not the matching content attribute: see
        // `DomBackend::form_value`. Read and written through the renderer's own
        // control state so what JavaScript sees is what is painted.
        "formValue" => Ok(Value::String(
            dom.form_value(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "setFormValue" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_form_value(node, bridge_arg(arguments, 1, "control value")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "formChecked" => Ok(Value::Bool(
            dom.form_checked(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "setFormChecked" => {
            let node = handle(runtime, arguments, 0)?;
            let checked = bridge_arg(arguments, 1, "control checkedness")? == "true";
            dom.set_form_checked(node, checked).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "styleGet" => Ok(Value::String(
            dom.inline_style(
                handle(runtime, arguments, 0)?,
                bridge_arg(arguments, 1, "property")?,
            )
            .map_err(dom_error)?
            .unwrap_or_default(),
        )),
        "styleSet" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inline_style(
                node,
                bridge_arg(arguments, 1, "property")?,
                bridge_arg(arguments, 2, "value")?,
            )
            .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "styleRemove" => Ok(Value::String(
            dom.remove_inline_style(
                handle(runtime, arguments, 0)?,
                bridge_arg(arguments, 1, "property")?,
            )
            .map_err(dom_error)?
            .unwrap_or_default(),
        )),
        "styleText" => Ok(Value::String(
            dom.inline_style_text(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "setStyleText" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inline_style_text(node, bridge_arg(arguments, 1, "CSS text")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "styleGetJs" => Ok(Value::String(
            dom.inline_style(
                handle(runtime, arguments, 0)?,
                &js_property_to_css(bridge_arg(arguments, 1, "property")?),
            )
            .map_err(dom_error)?
            .unwrap_or_default(),
        )),
        "styleSetJs" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inline_style(
                node,
                &js_property_to_css(bridge_arg(arguments, 1, "property")?),
                bridge_arg(arguments, 2, "value")?,
            )
            .map_err(dom_error)?;
            Ok(Value::Null)
        }
        // Layout-dependent like the geometry reads, and gated the same way: the
        // resolved value of a box property is the used value, which is only
        // knowable after style and layout have settled.
        "computedStyle" | "computedStyleJs" => {
            let forced = dom.layout_is_dirty();
            let node = handle(runtime, arguments, 0)?;
            let property = bridge_arg(arguments, 1, "property")?;
            let property = if operation == "computedStyleJs" {
                js_property_to_css(property)
            } else {
                property.to_owned()
            };
            let snapshot = dom.flush_layout().map_err(dom_error)?;
            let value = dom
                .resolved_style(node, &property, snapshot)
                .map_err(dom_error)?;
            Ok(json!({ "forced": forced, "value": value }))
        }
        "matchMedia" => {
            let query = dom
                .media_query(bridge_arg(arguments, 0, "media query")?)
                .map_err(dom_error)?;
            Ok(json!({ "media": query.media, "matches": query.matches }))
        }
        // `window.stop()`: every subresource still loading settles here, so a
        // stopped document paints what it has rather than waiting on requests
        // nobody is going to answer.
        "stopLoading" => Ok(json!(dom.stop_loading())),
        "documentUrl" => Ok(Value::String(web_url::DOCUMENT_URL.into())),
        "urlParts" => web_url::components(bridge_arg(arguments, 0, "URL")?).map_err(JsError::new),
        "resolveUrl" => web_url::resolve(
            bridge_arg(arguments, 0, "base URL")?,
            bridge_arg(arguments, 1, "URL")?,
        )
        .map_err(JsError::new),
        _ => Err(JsError::new(format!(
            "unknown DOM bridge operation: {operation}"
        ))),
    }
}

fn json_string(env: sys::napi_env, value: &Value) -> Result<Unknown<'static>, JsError> {
    let value = serde_json::to_string(value).map_err(|error| JsError::new(error.to_string()))?;
    let length = isize::try_from(value.len())
        .map_err(|_| JsError::new("DOM bridge result exceeds Node-API string limits"))?;
    let mut result = std::ptr::null_mut();
    check(
        unsafe { sys::napi_create_string_utf8(env, value.as_ptr().cast(), length, &mut result) },
        "serialize DOM bridge result",
    )?;
    Ok(unknown(env, result))
}

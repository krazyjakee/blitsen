//! Native DOM object installation for the Bun host.

use std::cell::RefCell;
use std::rc::Rc;

use blitsen_core::{WindowState, WrapperTable, js_property_to_css};
use blitsen_dom::{DomBackend, DomError, DomName, NodeKind};
use blitsen_js::{ExternalId, JsEngine, JsError, NativeClass};
use blitz::dom::NodeId;
use napi::{Env, Unknown, sys};
use serde_json::{Value, json};

use super::{DomRuntime, NodeApiEngine, NodeWeakRef, callback_string, check, unknown};

const BOOTSTRAP: &str = r#"
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
    return animationFrames.size;
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
    if (type === "click" && allowed) focusNearest(target);
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

  class Node extends EventTarget {
    constructor() { throw new TypeError("Illegal constructor"); }
    get nodeType() { return call("kind", this[handle]) === "element" ? 1 : 3; }
    get nodeName() { return this.nodeType === 1 ? this.tagName.toUpperCase() : '#text'; }
    get ownerDocument() { return document; }
    appendChild(child) {
      call("appendChild", this[handle], requireNode(child));
      notifyMutation({ type: "childList", target: this, addedNodes: new NodeList([child]),
        removedNodes: new NodeList([]), previousSibling: child.previousSibling, nextSibling: null });
      return child;
    }
    insertBefore(child, reference) {
      call("insertBefore", this[handle], requireNode(child), reference == null ? "" : requireNode(reference));
      notifyMutation({ type: "childList", target: this, addedNodes: new NodeList([child]),
        removedNodes: new NodeList([]), previousSibling: child.previousSibling, nextSibling: reference });
      return child;
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
    get childNodes() { return new NodeList(call("childNodes", this[handle]).map(wrap)); }
    get firstChild() { return wrap(call("firstChild", this[handle])); }
    get nextSibling() { return wrap(call("nextSibling", this[handle])); }
    get isConnected() { return call("isConnected", this[handle]); }
    get textContent() { return call("textContent", this[handle]); }
    set textContent(value) {
      call("setTextContent", this[handle], String(value));
      notifyMutation({ type: "characterData", target: this, oldValue: null });
    }
  }

  const styleCache = new WeakMap();
  const classListCache = new WeakMap();

  class Element extends Node {
    get tagName() { return elementTag(this).toUpperCase(); }
    get localName() { return elementTag(this); }
    get namespaceURI() { return "http://www.w3.org/1999/xhtml"; }
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
    get id() { return this.getAttribute("id") ?? ""; }
    set id(value) { this.setAttribute("id", value); }
    get className() { return this.getAttribute("class") ?? ""; }
    set className(value) { this.setAttribute("class", value); }
    get classList() {
      let list = classListCache.get(this);
      if (!list) {
        list = new DOMTokenList(this);
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

  class NodeList {
    constructor(items) {
      Object.defineProperty(this, "length", { value: items.length, enumerable: false });
      items.forEach((item, index) => Object.defineProperty(this, index, { value: item, enumerable: true }));
      Object.freeze(this);
    }
    item(index) { return this[index] ?? null; }
    *[Symbol.iterator]() { for (let index = 0; index < this.length; index++) yield this[index]; }
  }

  class DOMTokenList {
    constructor(element) { this._element = element; }
    _tokens() { return this._element.className.trim() ? this._element.className.trim().split(/\s+/) : []; }
    _validate(tokens) {
      for (const token of tokens) {
        if (!token || /\s/.test(token)) throw new DOMException("The token must not be empty or contain whitespace", "SyntaxError");
      }
    }
    contains(token) { this._validate([token]); return this._tokens().includes(token); }
    add(...tokens) {
      this._validate(tokens);
      const values = this._tokens();
      for (const token of tokens) if (!values.includes(token)) values.push(token);
      this._element.className = values.join(" ");
    }
    remove(...tokens) {
      this._validate(tokens);
      this._element.className = this._tokens().filter(token => !tokens.includes(token)).join(" ");
    }
    toggle(token, force) {
      this._validate([token]);
      const present = this.contains(token);
      const desired = force === undefined ? !present : Boolean(force);
      if (desired !== present) (desired ? this.add(token) : this.remove(token));
      return desired;
    }
    toString() { return this._element.className; }
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

  const requireNode = value => {
    if (!(value instanceof Node) || !(handle in value)) throw new TypeError("argument is not a Node");
    return value[handle];
  };
  const wrapperCache = new Map();
  const wrap = rawHandle => {
    if (rawHandle == null) return null;
    rawHandle = String(rawHandle);
    const cached = wrapperCache.get(rawHandle);
    if (cached) return cached;
    const wrapper = __blitsenWrap(rawHandle);
    if (!(handle in wrapper)) {
      Object.defineProperty(wrapper, handle, { value: rawHandle });
      Object.setPrototypeOf(wrapper, call("kind", rawHandle) === "element" ? Element.prototype : Node.prototype);
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
    getElementById(id) { return wrap(call("getElementById", String(id))); }
    createElement(name) { return wrap(call("createElement", String(name))); }
    createTextNode(text) { return wrap(call("createTextNode", String(text))); }
    get body() { return wrap(call("body")); }
    get head() { return this.querySelector("head"); }
    get documentElement() { return wrap(call("documentElement")); }
    get defaultView() { return globalThis; }
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
  class SVGElement {
    static [Symbol.hasInstance](value) {
      return value instanceof Element && value.namespaceURI === "http://www.w3.org/2000/svg";
    }
  }
  for (const method of ["addEventListener", "removeEventListener", "dispatchEvent"])
    Object.defineProperty(globalThis, method, { value: EventTarget.prototype[method], configurable: true });
  const globals = {
    EventTarget, Node, Element, NodeList, Document, DOMTokenList, CSSStyleDeclaration,
    MutationObserver, HTMLElement, HTMLIFrameElement, SVGElement, document,
    Event, MouseEvent, KeyboardEvent, CustomEvent,
    requestAnimationFrame, cancelAnimationFrame,
    setTimeout, clearTimeout, setInterval, clearInterval,
    __blitsenAnimationFrameTick: animationFrameTick,
    __blitsenAnimationFramesPending: () => animationFrames.size > 0,
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
      wrapperCache.clear();
      Object.assign(globalThis, {
        setTimeout: hostSetTimeout, clearTimeout: hostClearTimeout,
        setInterval: hostSetInterval, clearInterval: hostClearInterval,
      });
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
  for (const key of ["location", "history", "navigator", "localStorage"]) {
    try { delete globalThis[key]; } catch {}
  }
})();
"#;

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
    let dev_layout_warnings = std::env::var("BLITSEN_DEV_LAYOUT_WARNINGS").is_ok_and(|value| {
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    });
    let dev_layout_warnings = engine.boolean(dev_layout_warnings);
    engine.set_global("__blitsenDevLayoutWarnings", &dev_layout_warnings)?;
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
                "offsetWidth": metrics.offset_width,
                "offsetHeight": metrics.offset_height,
                "clientWidth": metrics.client_width,
                "clientHeight": metrics.client_height,
                "scrollLeft": metrics.scroll_left,
                "scrollTop": metrics.scroll_top,
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
        "createElement" => {
            let name = bridge_arg(arguments, 0, "element name")?;
            if name.is_empty()
                || name.chars().any(|character| {
                    character.is_whitespace() || matches!(character, '<' | '>' | '/' | '\0')
                })
            {
                return Err(JsError::new("invalid HTML element name"));
            }
            Ok(serialized(Some(
                dom.create_element(&DomName::html(name.to_ascii_lowercase()))
                    .map_err(dom_error)?,
            )))
        }
        "createTextNode" => Ok(serialized(Some(
            dom.create_text(bridge_arg(arguments, 0, "text")?)
                .map_err(dom_error)?,
        ))),
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
        "firstChild" => Ok(serialized(
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .first()
                .copied(),
        )),
        "nextSibling" => Ok(serialized(
            dom.next_sibling(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "isConnected" => Ok(Value::Bool(
            dom.is_connected(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "textContent" => Ok(Value::String(
            dom.text_content(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
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
        "setInnerHTML" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inner_html(node, bridge_arg(arguments, 1, "HTML")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
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

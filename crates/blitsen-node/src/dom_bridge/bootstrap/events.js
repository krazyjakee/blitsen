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
    // The legacy factory's other half. Ignored mid-dispatch, as the spec says,
    // so an event cannot be retyped while listeners are walking it.
    initEvent(type, bubbles = false, cancelable = false) {
      const state = stateFor(this);
      if (state.dispatching) return;
      state.type = String(type);
      state.bubbles = Boolean(bubbles);
      state.cancelable = Boolean(cancelable);
      state.defaultPrevented = false;
      state.propagationStopped = false;
      state.immediatePropagationStopped = false;
    }
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

  // A pointer is a mouse here: this runtime has one pointing device, it is
  // always primary, and it reports no tilt or pressure. The members are present
  // and truthful about that rather than absent, because a library reads
  // `pointerType` unguarded once it has decided to use pointer events at all.
  class PointerEvent extends MouseEvent {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperties(this, {
        pointerId: { value: Number(options.pointerId ?? 1), enumerable: true },
        pointerType: { value: String(options.pointerType ?? "mouse"), enumerable: true },
        isPrimary: { value: options.isPrimary === undefined ? true : Boolean(options.isPrimary), enumerable: true },
        width: { value: Number(options.width ?? 1), enumerable: true },
        height: { value: Number(options.height ?? 1), enumerable: true },
        pressure: { value: Number(options.pressure ?? 0), enumerable: true },
        tangentialPressure: { value: Number(options.tangentialPressure ?? 0), enumerable: true },
        tiltX: { value: Number(options.tiltX ?? 0), enumerable: true },
        tiltY: { value: Number(options.tiltY ?? 0), enumerable: true },
        twist: { value: Number(options.twist ?? 0), enumerable: true },
      });
    }
  }

  // `deltaX`/`deltaY` are already on `MouseEvent` because that is where the
  // native window's wheel input lands; this adds the two members that are only
  // a wheel's. `deltaMode` is 0 — pixels — because that is the unit the
  // platform delivers scrolling in, not a line or page count.
  class WheelEvent extends MouseEvent {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperties(this, {
        deltaZ: { value: Number(options.deltaZ ?? 0), enumerable: true },
        deltaMode: { value: Number(options.deltaMode ?? 0), enumerable: true },
      });
    }
  }

  class FocusEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperty(this, "relatedTarget",
        { value: options.relatedTarget ?? null, enumerable: true });
    }
  }

  // `data` is the text an input contributed and `inputType` how it got there.
  // Composition is never in progress: there is no IME path into this runtime,
  // so `isComposing` is false rather than unknown.
  class InputEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperties(this, {
        data: { value: options.data === undefined ? null : String(options.data), enumerable: true },
        inputType: { value: String(options.inputType ?? ""), enumerable: true },
        isComposing: { value: Boolean(options.isComposing), enumerable: true },
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
      // Configurable so initCustomEvent can replace it; a browser's detail is
      // likewise settable only through the initializer, never by assignment.
      Object.defineProperty(this, "detail",
        { value: options.detail ?? null, enumerable: true, configurable: true });
    }
    initCustomEvent(type, bubbles = false, cancelable = false, detail = null) {
      if (stateFor(this).dispatching) return;
      this.initEvent(type, bubbles, cancelable);
      Object.defineProperty(this, "detail", { value: detail, enumerable: true, configurable: true });
    }
  }

  // `MessageEvent` exists because `WebSocket` delivers one. The members a
  // message channel would fill are here and empty rather than absent: they are
  // the ones a library reads unguarded, and `source` and `ports` are truthfully
  // nothing when the message came off a socket.
  class MessageEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperties(this, {
        data: { value: options.data ?? null, enumerable: true },
        origin: { value: String(options.origin ?? ""), enumerable: true },
        lastEventId: { value: String(options.lastEventId ?? ""), enumerable: true },
        source: { value: options.source ?? null, enumerable: true },
        ports: { value: Object.freeze([...(options.ports ?? [])]), enumerable: true },
      });
    }
  }

  class CloseEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperties(this, {
        code: { value: options.code === undefined ? 0 : Number(options.code), enumerable: true },
        reason: { value: String(options.reason ?? ""), enumerable: true },
        wasClean: { value: Boolean(options.wasClean), enumerable: true },
      });
    }
  }

  // The legacy event factory, kept because framework helpers still reach for it —
  // Svelte's custom_event is createEvent + initCustomEvent. Only the interfaces
  // the DOM spec still requires are answered; anything else is refused rather
  // than handed back a differently-shaped event.
  const LEGACY_EVENT_INTERFACES = { event: Event, events: Event, htmlevents: Event,
    customevent: CustomEvent };

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
  // Four events, not two. `focus` and `blur` do not bubble, so a framework that
  // delegates from the root — React has since 17 — sees nothing unless the
  // bubbling `focusin`/`focusout` pair is dispatched as well. Each carries the
  // other end of the move as `relatedTarget`, which is what a component reads to
  // tell "focus left this subtree" from "focus moved inside it".
  const setFocus = element => {
    const next = element ?? document.body;
    const previous = activeElement ?? document.body;
    if (next === previous) { activeElement = next; return; }
    activeElement = next;
    previous?.dispatchEvent(new FocusEvent("blur", { relatedTarget: next }));
    next?.dispatchEvent(new FocusEvent("focus", { relatedTarget: previous }));
    previous?.dispatchEvent(new FocusEvent("focusout", { bubbles: true, relatedTarget: next }));
    next?.dispatchEvent(new FocusEvent("focusin", { bubbles: true, relatedTarget: previous }));
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
  // The native window reports gaining and losing focus as window-level `focus`
  // and `blur`. Nothing else changes it, so tracking it here is the whole of
  // what `document.hasFocus()` has to answer. A window that has not been told
  // otherwise is the focused one: it was just opened.
  let windowFocused = true;
  const windowHasFocus = () => windowFocused;
  const dispatchLifecycleEvent = type => {
    if (type === "focus" || type === "blur") windowFocused = type === "focus";
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


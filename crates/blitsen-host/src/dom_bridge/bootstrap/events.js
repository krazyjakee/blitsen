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
      // The browsing context the event belongs to. D3 reads this to install
      // its temporary move/up listeners on the window during a drag.
      const members = { view: options.view ?? null };
      for (const property of ["clientX", "clientY", "offsetX", "offsetY", "screenX", "screenY",
        "button", "buttons", "deltaX", "deltaY"]) members[property] = Number(options[property] ?? 0);
      for (const property of ["ctrlKey", "shiftKey", "altKey", "metaKey"])
        members[property] = Boolean(options[property]);
      defineMembers(this, members);
    }
  }

  // A pointer is a mouse here: this runtime has one pointing device, it is
  // always primary, and it reports no tilt or pressure. The members are present
  // and truthful about that rather than absent, because a library reads
  // `pointerType` unguarded once it has decided to use pointer events at all.
  class PointerEvent extends MouseEvent {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, {
        pointerId: Number(options.pointerId ?? 1),
        pointerType: String(options.pointerType ?? "mouse"),
        isPrimary: options.isPrimary === undefined ? true : Boolean(options.isPrimary),
        width: Number(options.width ?? 1),
        height: Number(options.height ?? 1),
        pressure: Number(options.pressure ?? 0),
        tangentialPressure: Number(options.tangentialPressure ?? 0),
        tiltX: Number(options.tiltX ?? 0),
        tiltY: Number(options.tiltY ?? 0),
        twist: Number(options.twist ?? 0),
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
      defineMembers(this, {
        deltaZ: Number(options.deltaZ ?? 0),
        deltaMode: Number(options.deltaMode ?? 0),
      });
    }
  }

  class FocusEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, { relatedTarget: options.relatedTarget ?? null });
    }
  }

  // `data` is the text an input contributed and `inputType` how it got there.
  // Composition is never in progress: there is no IME path into this runtime,
  // so `isComposing` is false rather than unknown.
  class InputEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, {
        // Nullable rather than merely optional: a deletion contributes no text
        // and says so with null, which `String()` would hand a listener as the
        // four characters "null".
        data: options.data === undefined || options.data === null
          ? null : String(options.data),
        inputType: String(options.inputType ?? ""),
        isComposing: Boolean(options.isComposing),
      });
    }
  }

  // Monaco and other mature keyboard-driven widgets still use the deprecated
  // numeric keyCode/which pair to resolve keybindings. Native events have both
  // modern identities already, so derive the legacy value at the boundary
  // instead of making every application carry a compatibility listener.
  const legacyKeyCode = (key, code) => {
    if (/^Key[A-Z]$/.test(code)) return code.charCodeAt(3);
    if (/^Digit[0-9]$/.test(code)) return code.charCodeAt(5);
    if (/^Numpad[0-9]$/.test(code)) return 96 + Number(code.slice(-1));
    const functionKey = /^F([1-9]|1[0-9]|2[0-4])$/.exec(code);
    if (functionKey !== null) return 111 + Number(functionKey[1]);
    return {
      Backspace: 8, Tab: 9, Enter: 13, NumpadEnter: 13,
      ShiftLeft: 16, ShiftRight: 16, ControlLeft: 17, ControlRight: 17,
      AltLeft: 18, AltRight: 18, Pause: 19, CapsLock: 20, Escape: 27,
      Space: 32, PageUp: 33, PageDown: 34, End: 35, Home: 36,
      ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40,
      Insert: 45, Delete: 46, MetaLeft: 91, MetaRight: 92, ContextMenu: 93,
      NumpadMultiply: 106, NumpadAdd: 107, NumpadComma: 108,
      NumpadSubtract: 109, NumpadDecimal: 110, NumpadDivide: 111,
      NumLock: 144, ScrollLock: 145, Semicolon: 186, Equal: 187,
      Comma: 188, Minus: 189, Period: 190, Slash: 191, Backquote: 192,
      BracketLeft: 219, Backslash: 220, BracketRight: 221, Quote: 222,
    }[code] ?? {
      Backspace: 8, Tab: 9, Enter: 13, Shift: 16, Control: 17, Alt: 18,
      Pause: 19, CapsLock: 20, Escape: 27, " ": 32, Space: 32,
      PageUp: 33, PageDown: 34, End: 35, Home: 36, ArrowLeft: 37,
      ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Insert: 45, Delete: 46,
      Meta: 91, ContextMenu: 93, NumLock: 144, ScrollLock: 145,
    }[key] ?? 0;
  };

  class KeyboardEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      const key = String(options.key ?? "");
      const code = String(options.code ?? "");
      const keyCode = legacyKeyCode(key, code);
      defineMembers(this, {
        key,
        code,
        keyCode,
        which: keyCode,
        charCode: 0,
        repeat: Boolean(options.repeat),
        ctrlKey: Boolean(options.ctrlKey),
        shiftKey: Boolean(options.shiftKey),
        altKey: Boolean(options.altKey),
        metaKey: Boolean(options.metaKey),
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

  class CloseEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, {
        code: options.code === undefined ? 0 : Number(options.code),
        reason: String(options.reason ?? ""),
        wasClean: Boolean(options.wasClean),
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
      defineMembers(this, { submitter: options.submitter ?? null });
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
    // The renderer paints from its own idea of focus — the caret in a field,
    // the highlight behind a selection, every `:focus` rule — and is told here
    // because this is where the decision is made. The body goes as nothing: it
    // is where HTML parks focus when no control holds it, and `:focus` matches
    // on neither.
    call("setFocusedNode", next === document.body ? "" : next[handle]);
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
    const event = new MouseEvent(String(type), { ...init, view: init.view ?? globalThis });
    const allowed = target.dispatchEvent(event);
    // Focus is `mousedown`'s default action and activation is `click`'s. They
    // are two different events on purpose: a component that has focused
    // something of its own — a code editor moving the caret into its hidden
    // textarea — cancels the mousedown to keep it, and by the time a click has
    // happened there is nothing left to cancel. Taking focus at click instead
    // handed it straight back to the nearest focusable ancestor, or to the body
    // when there was none, one event after the application had placed it.
    if (type === "mousedown" && allowed) focusNearest(target);
    if (type === "click" && allowed) activateControl(target);
    if (allowed) textEditingMouse(type, target, event);
    if (type === "wheel" && allowed)
      __blitsenScrollDefault(String(target[handle]), String(-event.deltaX), String(-event.deltaY));
    return allowed;
  };
  const dispatchKeyboardEvent = (type, init) => {
    const event = new KeyboardEvent(String(type), init);
    const target = activeElement ?? document.body ?? document;
    const allowed = target.dispatchEvent(event);
    if (type === "keydown" && init.key === "Tab" && allowed) moveFocus(Boolean(init.shiftKey));
    // A key the focused field took is not also a scroll: a space typed into it
    // must not page the document down behind it, and Home must not leave the
    // caret behind at the top of it.
    if (type === "keydown" && allowed && textEditingKeydown(event, target)) return allowed;
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

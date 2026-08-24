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
        "movementX", "movementY", "button", "buttons", "deltaX", "deltaY"])
        members[property] = Number(options[property] ?? 0);
      for (const property of ["ctrlKey", "shiftKey", "altKey", "metaKey"])
        members[property] = Boolean(options[property]);
      defineMembers(this, members);
    }
  }

  // Every member is filled from the device the host reported, so `pointerType`
  // is "mouse", "touch" or "pen" as the platform saw it, `pointerId` is stable
  // for the life of one contact, and `pressure` is the force a touchscreen or a
  // tablet measured. `width` and `height` are the contact geometry, and stay 1:
  // winit reports no touch-ellipse, and a guessed one would be a measurement
  // this runtime never made.
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

  class CompositionEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, {
        data: String(options.data ?? ""),
        locale: String(options.locale ?? ""),
      });
    }
  }

  // `data` is the text an input contributed and `inputType` how it got there.
  // `isComposing` is supplied by the native IME path for edits between
  // `compositionstart` and `compositionend`; ordinary keyboard edits omit it
  // and therefore remain false.
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

  // Incremented by every JavaScript-side tree mutation. A native hit path is a
  // snapshot of the tree immediately before dispatch; compatibility mouse and
  // click events may reuse it only while no listener has changed that tree.
  let treeRevision = 0;
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

  // Turns the root-to-target handles returned by the native hit test into the
  // event path without reading parentNode or isConnected back through the JSON
  // bridge. The first handle is the backing document node; JavaScript exposes
  // the singleton `document` instead of a wrapper for that node.
  const makePropagationHint = (target, rawPath) => {
    if (!Array.isArray(rawPath) || rawPath.length < 2 || !(target instanceof Node)) return null;
    if (String(rawPath[rawPath.length - 1]) !== String(target[handle])) return null;
    const path = [globalThis, document, ...rawPath.slice(1).map(wrap)];
    return path[path.length - 1] === target ? { revision: treeRevision, path } : null;
  };
  const hintedPropagationPath = (target, hint) =>
    hint?.revision === treeRevision && hint.path[hint.path.length - 1] === target
      ? hint.path : null;

  const dispatchTo = (target, event, hint = null) => {
    if (!(event instanceof Event)) throw new TypeError("dispatchEvent argument must be an Event");
    const state = stateFor(event);
    if (state.dispatching) throw new DOMException("The event is already being dispatched", "InvalidStateError");
    state.dispatching = true;
    state.target = target;
    state.propagationStopped = false;
    state.immediatePropagationStopped = false;
    const path = hintedPropagationPath(target, hint) ?? propagationPath(target);
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
  // Filled by text_editing.js once the form-control helpers exist. Focus is
  // defined earlier in the bootstrap, but moving it must synchronously discard
  // a preedit from the control that is about to blur.
  let cancelTextComposition = () => {};
  let readyState = "loading";
  const elementTag = element => call("tagName", element[handle]);
  const isFocusable = element => element instanceof Element && call("isFocusable", element[handle]);
  // Four events, not two. `focus` and `blur` do not bubble, so a framework that
  // delegates from the root — React has since 17 — sees nothing unless the
  // bubbling `focusin`/`focusout` pair is dispatched as well. Each carries the
  // other end of the move as `relatedTarget`, which is what a component reads to
  // tell "focus left this subtree" from "focus moved inside it".
  const setFocus = element => {
    const next = element ?? document.body;
    const previous = activeElement ?? document.body;
    if (next === previous) { activeElement = next; return; }
    cancelTextComposition();
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
    const next = call("nextFocusable", activeElement?.[handle] ?? "", Boolean(backwards));
    setFocus(next === null ? document.body : wrap(next));
  };
  // The DOM `pointerId` the mouse always has, matching the host's constant. Not
  // 0: the spec reserves that for a pointer of unknown identity.
  const MOUSE_POINTER_ID = 1;
  // `MouseEvent.buttons` is a bitmask in a different order from `button`:
  // primary 1, secondary 2, auxiliary 4, then back and forward.
  const DOM_BUTTON_MASKS = [1, 4, 2, 8, 16];
  const buttonMask = button =>
    DOM_BUTTON_MASKS[button] ?? (button >= 0 && button < 16 ? 1 << button : 0);

  // One entry per pointer the platform is currently tracking. The host allocates
  // the ids and says which device is behind each; what is held here is the state
  // that only the DOM knows about — which buttons are down under this pointer,
  // which node each of them went down on, and whether this contact's
  // compatibility mouse events were refused.
  //
  // Per pointer rather than global, because that is the whole of multi-touch:
  // two fingers pressing two different elements are two independent presses, and
  // one shared "the mouse is down on X" would make the second lift click the
  // first finger's target.
  const activePointers = new Map();
  // The capture in effect, and the one requested. Two maps because capture is
  // *pending* until the next pointer event: an element that captures from a
  // `pointerdown` handler is still not the target of that same event, and
  // `gotpointercapture` has not fired yet when the handler returns.
  const pointerCaptures = new Map();
  const pendingPointerCaptures = new Map();

  const pointerStateFor = pointerId => {
    let state = activePointers.get(pointerId);
    if (!state) {
      state = { buttons: 0, downTargets: new Map(), compatibilitySuppressed: false };
      activePointers.set(pointerId, state);
    }
    return state;
  };

  // Pointer lock holds these coordinates constant while raw device movement is
  // reported separately as movementX/Y. The last absolute event is the least
  // surprising fixed point and is also what a browser preserves on entry.
  let lastMousePosition = { clientX: 0, clientY: 0, screenX: 0, screenY: 0 };

  const dispatchMouseEvent = (type, rawHandle, init, inheritedHint = null) => {
    if (pointerLockElement === null) {
      lastMousePosition = {
        clientX: Number(init.clientX ?? 0), clientY: Number(init.clientY ?? 0),
        screenX: Number(init.screenX ?? 0), screenY: Number(init.screenY ?? 0),
      };
    }
    const rawTarget = wrap(String(rawHandle));
    const hint = inheritedHint ?? makePropagationHint(rawTarget, init.propagationPath);
    const target = pointerLockElement ?? rawTarget;
    // `buttons` is the pointer's state rather than this event's, so an event
    // that does not carry one — a wheel, which no pointer produced — reads it
    // off the mouse pointer instead of reporting nothing held.
    const event = new MouseEvent(String(type), { ...init, view: init.view ?? globalThis,
      buttons: init.buttons ?? (activePointers.get(MOUSE_POINTER_ID)?.buttons ?? 0) });
    const allowed = dispatchTo(target, event, hint);
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

  const pointerCaptureSet = (element, pointerId) => {
    const id = Number(pointerId);
    if (!activePointers.has(id))
      throw new DOMException(`no active pointer with pointerId ${id}`, "NotFoundError");
    if (!element.isConnected)
      throw new DOMException("cannot capture a pointer on a disconnected element", "InvalidStateError");
    pendingPointerCaptures.set(id, element);
  };
  const pointerCaptureRelease = (element, pointerId) => {
    const id = Number(pointerId);
    if (!activePointers.has(id))
      throw new DOMException(`no active pointer with pointerId ${id}`, "NotFoundError");
    if (pendingPointerCaptures.get(id) === element) pendingPointerCaptures.delete(id);
  };
  const pointerCaptureHas = (element, pointerId) =>
    pendingPointerCaptures.get(Number(pointerId)) === element;

  // Settles a requested capture, immediately before the pointer event that will
  // be retargeted by it. `lostpointercapture` goes to the element that had it and
  // `gotpointercapture` to the one taking it, in that order, and neither is
  // cancelable — the transfer has already happened by the time they are seen.
  //
  // A captured element that has left the document releases here rather than
  // holding the pointer for ever: a drag handle a re-render replaced would
  // otherwise swallow every move for the rest of the gesture.
  const processPendingPointerCapture = (pointerId, members) => {
    const requested = pendingPointerCaptures.get(pointerId) ?? null;
    const pending = requested !== null && requested.isConnected ? requested : null;
    if (pending === null && requested !== null) pendingPointerCaptures.delete(pointerId);
    const active = pointerCaptures.get(pointerId) ?? null;
    if (pending === active) return;
    const notify = (type, target) =>
      target.dispatchEvent(new PointerEvent(type, { ...members, bubbles: true, cancelable: false }));
    if (active !== null) { pointerCaptures.delete(pointerId); notify("lostpointercapture", active); }
    if (pending !== null) { pointerCaptures.set(pointerId, pending); notify("gotpointercapture", pending); }
  };

  // The mouse event a browser still synthesises behind each pointer event.
  const COMPATIBILITY_MOUSE_EVENT =
    { pointerdown: "mousedown", pointermove: "mousemove", pointerup: "mouseup" };

  // A pointer event, and the mouse event synthesised behind it.
  //
  // Both are dispatched, pointer first, and that is deliberate rather than
  // transitional. Every browser does it, and the reason is that the mouse events
  // are what the installed base listens for: a component written against
  // `mousedown`/`click` — which is most of them, and all of the ones already
  // running on Blitsen — must keep working when the press came from a finger.
  // Dispatching only pointer events would make touch input work and break every
  // application that already runs; dispatching only mouse events is what Blitsen
  // did before, and it threw away the pointer's identity and pressure.
  //
  // Two rules keep the pair from being noise. Only the *primary* pointer
  // synthesises them, so a second finger does not fire a second `mousedown` at
  // whatever it landed on; and a cancelled `pointerdown` suppresses them for the
  // rest of that contact, which is how an application takes over a gesture.
  const dispatchPointerEvent = (type, rawHandle, init) => {
    const pointerId = Number(init.pointerId ?? MOUSE_POINTER_ID);
    const pointerType = String(init.pointerType ?? "mouse");
    const isPrimary = init.isPrimary === undefined ? true : Boolean(init.isPrimary);
    // Remember the absolute point before a pointerdown listener can lock it;
    // the compatibility mousedown is dispatched after that listener returns.
    if (pointerLockElement === null && pointerType === "mouse") {
      lastMousePosition = {
        clientX: Number(init.clientX ?? 0), clientY: Number(init.clientY ?? 0),
        screenX: Number(init.screenX ?? 0), screenY: Number(init.screenY ?? 0),
      };
    }
    // Activation begins at the trusted press. A release is not a new gesture.
    if (isPrimary && type === "pointerdown")
      grantWindowModeActivation();
    // `button` names the button that *changed*, which on a move or a
    // cancellation is none of them. That is a property of the event type rather
    // than of the input, so it is settled here and not left to the caller.
    const button = type === "pointermove" || type === "pointercancel"
      ? -1 : Number(init.button ?? 0);
    const state = pointerStateFor(pointerId);
    if (type === "pointerdown") {
      state.buttons |= buttonMask(button);
      state.compatibilitySuppressed = false;
    } else if (type === "pointerup") {
      state.buttons &= ~buttonMask(button);
    } else if (type === "pointercancel") {
      state.buttons = 0;
      state.downTargets.clear();
    }
    // The force the device measured, or the value the spec substitutes for
    // hardware that cannot measure one: 0.5 while a button is held and 0
    // otherwise. A lift and a cancellation are 0 either way — nothing is
    // pressing any more.
    const pressure = type === "pointerup" || type === "pointercancel" ? 0
      : init.force === undefined || init.force === null
        ? (state.buttons === 0 ? 0 : 0.5)
        : Number(init.force);
    const members = { ...init, view: init.view ?? globalThis,
      bubbles: true, cancelable: type !== "pointercancel",
      pointerId, pointerType, isPrimary, pressure, button, buttons: state.buttons };
    processPendingPointerCapture(pointerId, members);
    const rawTarget = wrap(String(rawHandle));
    const hint = makePropagationHint(rawTarget, init.propagationPath);
    const target = pointerLockElement ?? pointerCaptures.get(pointerId) ?? rawTarget;
    const allowed = dispatchTo(target, new PointerEvent(String(type), members), hint);
    if (type === "pointerdown" && !allowed) state.compatibilitySuppressed = true;
    const compatibility = COMPATIBILITY_MOUSE_EVENT[type];
    const synthesise = isPrimary && !state.compatibilitySuppressed;
    if (compatibility && synthesise)
      // `button` on a move is the button that changed, which is none of them:
      // -1 to a pointer event and 0 to the mouse event, where the interfaces
      // simply disagree and both spellings are the correct one.
      dispatchMouseEvent(compatibility, String(target[handle]),
        { ...members, button: type === "pointermove" ? 0 : button }, hint);
    if (type === "pointerdown") state.downTargets.set(button, target);
    if (type === "pointerup") {
      const pressed = state.downTargets.get(button);
      state.downTargets.delete(button);
      // Activation, at the element the press and the lift agree on. Under
      // capture that is the capturing element for both, which is what makes a
      // drag that ends outside its handle still a click on it.
      if (button === 0 && pressed === target && synthesise)
        dispatchMouseEvent("click", String(target[handle]), members, hint);
    }
    if (type === "pointerup" || type === "pointercancel") {
      // Capture is released implicitly when the contact ends, after the event
      // that ended it — so a `pointerup` handler still sees the captured target.
      pendingPointerCaptures.delete(pointerId);
      processPendingPointerCapture(pointerId, members);
      // A finger that has lifted no longer exists; a mouse that released a
      // button is still on the screen and can still capture.
      if (pointerType !== "mouse") activePointers.delete(pointerId);
    }
    return allowed;
  };

  const disposePointerState = () => {
    activePointers.clear();
    pointerCaptures.clear();
    pendingPointerCaptures.clear();
  };

  const dispatchKeyboardEvent = (type, init) => {
    // Escape is an always-available user-agent exit. Native restoration and
    // DOM state clearing happen before application listeners; preventDefault
    // cannot retain either security-sensitive mode.
    if (type === "keydown" && init.key === "Escape" && !init.repeat) {
      releasePointerLock(false, "escape");
      void releaseFullscreen(false, "escape");
    }
    if (type === "keydown" && init.key !== "Escape" && !init.repeat)
      grantWindowModeActivation();
    const event = new KeyboardEvent(String(type), init);
    const target = activeElement ?? document.body ?? document;
    const allowed = target.dispatchEvent(event);
    if (type === "keydown" && init.key === "Tab" && allowed) moveFocus(Boolean(init.shiftKey));
    // Before the field's own default action, because Ctrl+X is not a character
    // to type and the cut it performs is an edit the field must not also make.
    if (type === "keydown" && allowed && clipboardShortcut(event, target)) return allowed;
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

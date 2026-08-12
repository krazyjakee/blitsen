  // Bound to the window. An unqualified call from an ES module has `this ===
  // undefined`, and a browser substitutes the global for a WebIDL operation on
  // Window — an unbound function would instead fail inside the listener table on
  // `addEventListener("load", …)`, which is the first line of a great many
  // entry scripts.
  for (const method of ["addEventListener", "removeEventListener", "dispatchEvent"])
    Object.defineProperty(globalThis, method,
      { value: EventTarget.prototype[method].bind(globalThis), configurable: true });
  const globals = {
    EventTarget, Node, Element, NodeList, Document, DocumentFragment, DOMTokenList,
    Attr, NamedNodeMap,
    CSSStyleDeclaration, MutationObserver, ResizeObserver, HTMLElement, HTMLIFrameElement,
    SVGElement, Text, Comment, Image,
    CSSStyleSheet, StyleSheetList, CSSRule, CSSRuleList, HTMLStyleElement,
    HTMLImageElement, HTMLLinkElement, HTMLTemplateElement, Storage, Navigator, document,
    HTMLInputElement, HTMLTextAreaElement, HTMLSelectElement, HTMLOptionElement,
    HTMLButtonElement, HTMLFormElement,
    BlitsenViewElement, BlitsenViewSurface,
    getComputedStyle, matchMedia, MediaQueryList, MediaQueryListEvent,
    Event, MouseEvent, KeyboardEvent, CustomEvent, SubmitEvent, PopStateEvent, HashChangeEvent,
    MessageEvent, CloseEvent,
    Headers, Request, Response, Blob, AbortController, AbortSignal, fetch, stop, WebSocket,
    Location, History,
    requestAnimationFrame, cancelAnimationFrame,
    setTimeout, clearTimeout, setInterval, clearInterval,
    __blitsenAnimationFrameTick: animationFrameTick,
    __blitsenAnimationFramesPending: () =>
      animationFrames.size > 0 || inflightFetches.size > 0 || liveSockets.size > 0
      || pendingResizeObservations() > 0
      || waitingImages() > 0 || nativePending() || nativeDialogPending() || call("isAnimating"),
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
      liveSockets.clear();
      dialogs.clear();
      secondInstanceHandler = null;
      __blitsenFetchDispose();
      __blitsenSocketDispose();
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
    "EventSource", "XMLHttpRequest",
    "ReadableStream", "WritableStream", "TransformStream",
    "FormData", "File", "FileReader",
    "HTMLCanvasElement", "CanvasRenderingContext2D", "OffscreenCanvas", "ImageData", "Path2D",
    "WebGLRenderingContext", "WebGL2RenderingContext", "GPUCanvasContext",
    "Audio", "AudioContext", "webkitAudioContext", "HTMLMediaElement",
    "alert", "confirm", "prompt", "print",
    "open", "close", "navigation",
    "cookieStore", "screen", "Notification", "caches",
    "IntersectionObserver", "PerformanceObserver",
    "CSSStyleRule", "CSSKeyframesRule", "CSSKeyframeRule", "CSSMediaRule",
    "customElements", "ShadowRoot", "DOMParser"]) {
    try { delete globalThis[key]; } catch {}
  }

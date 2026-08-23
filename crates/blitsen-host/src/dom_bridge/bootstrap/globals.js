  // Bound to the window. An unqualified call from an ES module has `this ===
  // undefined`, and a browser substitutes the global for a WebIDL operation on
  // Window — an unbound function would instead fail inside the listener table on
  // `addEventListener("load", …)`, which is the first line of a great many
  // entry scripts.
  for (const method of ["addEventListener", "removeEventListener", "dispatchEvent"])
    Object.defineProperty(globalThis, method,
      { value: EventTarget.prototype[method].bind(globalThis), configurable: true });
  // Document scrolling, under both the option-bag and the two-argument
  // spellings. The scrolling element is where the document's offsets live, so
  // moving the window is writing to it — there is no second scroll position to
  // keep in step. `behavior` is accepted and ignored, as it is on
  // `scrollIntoView`: the scroll lands rather than animating to its target.
  const scrollArguments = (first, second) => typeof first === "object" && first !== null
    ? { left: first.left, top: first.top } : { left: first, top: second };
  const scrollTo = (first, second) => {
    const { left, top } = scrollArguments(first, second);
    const element = document.scrollingElement;
    if (left !== undefined) element.scrollLeft = Number(left);
    if (top !== undefined) element.scrollTop = Number(top);
  };
  const scrollBy = (first, second) => {
    const { left, top } = scrollArguments(first, second);
    const element = document.scrollingElement;
    if (left !== undefined) element.scrollLeft += Number(left);
    if (top !== undefined) element.scrollTop += Number(top);
  };
  // A worker script is named relative to the document, exactly as any other
  // subresource is, and a message from another context in this application
  // reports the one origin there is.
  const resolveWorkerUrl = url => resolveAgainstDocument(url).href;
  const messageOrigin = resolveAgainstDocument("/").origin;
  // `window.postMessage`, which is the same-window one: a message to yourself,
  // delivered in a later task. `targetOrigin` is accepted and ignored — there is
  // a single origin behind an application, so every value of it either matches
  // or names somewhere that does not exist.
  const windowPostMessage = (message, targetOrigin = "*", transfer = []) => {
    // Serialized at the call and deserialized at delivery, so a later mutation
    // of the object posted is not visible to the listener. That ordering is the
    // whole reason schedulers use this as a yield.
    const copy = structuredClone(message, { transfer });
    hostSetTimeout(() => globalThis.dispatchEvent(new MessageEvent("message",
      { data: copy, origin: messageOrigin, source: globalThis })), 0);
  };
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
    HTMLCanvasElement, CanvasRenderingContext2D, ImageData, Path2D, DOMMatrix,
    CanvasGradient, CanvasPattern, TextMetrics,
    Range, Selection, CaretPosition, getSelection,
    getComputedStyle, matchMedia, MediaQueryList, MediaQueryListEvent,
    Event, MouseEvent, KeyboardEvent, CustomEvent, SubmitEvent, PopStateEvent, HashChangeEvent,
    MessageEvent, CloseEvent, ErrorEvent, FocusEvent, InputEvent, PointerEvent, WheelEvent,
    Worker, MessagePort, MessageChannel, structuredClone,
    postMessage: windowPostMessage,
    CSS, DOMParser,
    scrollTo, scrollBy, scroll: scrollTo,
    Headers, Request, Response, Blob, AbortController, AbortSignal, fetch, stop, WebSocket,
    EventSource, Notification,
    AudioContext, AudioNode, AudioParam, AudioBuffer, AudioBufferSourceNode, AudioDestinationNode,
    GainNode, StereoPannerNode, Audio, HTMLAudioElement,
    Location, History, URL, URLSearchParams,
    Intl,
    requestAnimationFrame, cancelAnimationFrame,
    setTimeout, clearTimeout, setInterval, clearInterval,
    __blitsenHostUrl: hostUrl,
    __blitsenAnimationFrameTick: animationFrameTick,
    __blitsenAnimationFramesPending: () =>
      animationFrames.size > 0 || inflightFetches.size > 0 || liveSockets.size > 0
      || liveEventSources.size > 0
      || pendingResizeObservations() > 0 || audioPending()
      || waitingImages() > 0 || waitingLinks() > 0
      || nativePending() || nativeDialogPending() || nativeTrayWorkPending()
      || nativeNotifyWorkPending() || call("isAnimating")
      // A canvas drawn outside a frame callback is owed a paint, and nothing
      // else here would ask for one.
      || canvasPaintPending
      // A message from a worker lands in the frame turn, so a loop that idled
      // would never deliver it — the same reason an open socket is listed.
      || portsPending(),
    __blitsenForcedLayoutsThisFrame: () => forcedLayoutsThisFrame,
    __blitsenEventInternals: eventInternals,
    __blitsenDispatchMouseEvent: dispatchMouseEvent,
    __blitsenDispatchPointerEvent: dispatchPointerEvent,
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
      drawnCanvases.clear();
      canvasPaintPending = false;
      pendingImages.clear();
      inflightFetches.clear();
      liveSockets.clear();
      liveEventSources.clear();
      livePorts.clear();
      liveWorkers.clear();
      dialogs.clear();
      // A press held across a reload would otherwise keep the old document's
      // field alive to drag a selection in, and a pointer captured by an element
      // of the old document would retarget the new document's events at it.
      caretDragControl = null;
      disposePointerState();
      secondInstanceHandler = null;
      notifyCommands.clear();
      notifyListeners.clear();
      __blitsenFetchDispose();
      __blitsenSocketDispose();
      __blitsenEventSourceDispose();
      // Ends the worker threads this document started. A reload that left them
      // running would leave the new document's messages arriving at the old
      // document's workers.
      __blitsenMessagingDispose();
      __blitsenAudioDispose();
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
  if (testHarness) globals.__blitsenDomCallCount = operation =>
    bridgeCallCounts.get(String(operation)) ?? 0;
  // Injects at viewport coordinates the way the native window does: hit test the
  // laid-out tree first, then dispatch at whatever that resolves to. Harness-only,
  // so tests exercise the same path as real input rather than picking a target.
  //
  // A `pointer*` type goes through the pointer dispatcher and a `mouse*` type
  // through the mouse one, because those are the two entry points the native
  // window itself calls: the pointer one for every contact, the mouse one for
  // the wheel. Injecting a mouse event directly is therefore *not* a shortcut
  // to the pointer path — it is the wheel's path, and the compatibility mouse
  // events a pointer synthesises are reached only through `pointerdown` and
  // its siblings.
  if (testHarness) globals.__blitsenInjectPointerAt = (type, clientX, clientY, init = {}) => {
    const hit = call("hitTest", Number(clientX), Number(clientY));
    if (!hit) return null;
    const members = {
      bubbles: true, cancelable: true,
      clientX: Number(clientX), clientY: Number(clientY),
      screenX: Number(clientX), screenY: Number(clientY),
      offsetX: hit.offsetX, offsetY: hit.offsetY,
      button: 0, ...init,
    };
    const allowed = String(type).startsWith("pointer")
      ? dispatchPointerEvent(String(type), hit.target, members)
      : dispatchMouseEvent(String(type), hit.target,
        { buttons: type === "mousedown" ? 1 : 0, ...members });
    return { allowed, target: wrap(hit.target), path: hit.path.map(wrap) };
  };
  Object.assign(globalThis, globals);
  globalThis.window = globalThis;
  // `self` is the global under the name code shares with a worker, which is why
  // library configuration is written through it — `self.MonacoEnvironment` is
  // how Monaco is told where its workers are. A worker scope already answers to
  // it; a document that did not made the same line a ReferenceError here and
  // fine there.
  globalThis.self = globalThis;
  // The document's scroll offsets, under all four names the platform has given
  // them. Accessors rather than values: `pageYOffset` is the same live reading
  // as `scrollY`, not a copy taken when the bridge was installed.
  for (const [name, axis] of [["scrollX", "scrollLeft"], ["pageXOffset", "scrollLeft"],
    ["scrollY", "scrollTop"], ["pageYOffset", "scrollTop"]])
    Object.defineProperty(globalThis, name, {
      get: () => document.scrollingElement[axis], enumerable: true, configurable: true });
  // `Intl` is a language global rather than a document one, so its three
  // prototype methods are installed over the engine's locale-blind versions at
  // the same moment the object itself appears.
  installIntlPrototypes();

  // Absent, not stubbed: an unimplemented API must not exist, so feature
  // detection selects a fallback. The Phase 1 host supplies several of these
  // itself, and leaving those in place would make them disappear at the Phase 2
  // engine swap. `packages/blitsen/src/api-manifest.mjs` reads this list, and
  // refuses to generate a manifest that describes any other API as absent.
  for (const key of ["requestIdleCallback", "cancelIdleCallback", "indexedDB",
    "SharedWorker", "ServiceWorker", "ServiceWorkerContainer",
    "BroadcastChannel",
    "XMLHttpRequest",
    "ReadableStream", "WritableStream", "TransformStream",
    "FormData", "File", "FileReader",
    "OffscreenCanvas", "ImageBitmap", "createImageBitmap", "OffscreenCanvasRenderingContext2D",
    "WebGLRenderingContext", "WebGL2RenderingContext", "GPUCanvasContext",
    "webkitAudioContext", "HTMLMediaElement",
    "alert", "confirm", "prompt", "print",
    "open", "close", "navigation",
    "cookieStore", "screen", "caches",
    "IntersectionObserver", "PerformanceObserver",
    "CSSStyleRule", "CSSKeyframesRule", "CSSKeyframeRule", "CSSMediaRule",
    // Custom elements stay absent by decision rather than by omission. Upgrading
    // an element after it is parsed, running the lifecycle callbacks and
    // ordering reactions is a real piece of machinery, and `<blitsen-view>` is
    // registered natively rather than through a registry — so a user-defined
    // element would either need its own registry beside that one or a merge of
    // the two. Neither is worth doing before something measured asks for it, and
    // an absent `customElements` is a polyfill a library installs itself.
    "customElements", "ShadowRoot"]) {
    try { delete globalThis[key]; } catch {}
  }
  if (!Notification) try { delete globalThis.Notification; } catch {}

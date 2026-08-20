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
  // The host's own `URL`, kept before the bridge installs Blitsen's over it.
  // Object URLs belong to the host rather than to the application — there is no
  // origin behind one to hang a `blob:` on — and the Phase 1 loader needs them
  // to evaluate an inline module (`blitsen-node/src/engine.rs`). Absent on the
  // Phase 2 host, which has no URL of its own and no such loader.
  // Whatever was captured the first time this bootstrap ran, because by the
  // second document the global is already Blitsen's.
  const hostUrl = globalThis.__blitsenHostUrl ?? globalThis.URL;
  // The indexed half of a collection interface — `NodeList`, `NamedNodeMap`,
  // `CSSRuleList`, `StyleSheetList`, which are four constructors of the same
  // object. Entries sit at numeric keys, `length` is not enumerable, and the
  // whole thing is frozen, which together are what make `list[0]`, `[...list]`
  // and `Object.keys(list)` agree with each other and with a browser.
  //
  // A snapshot, not a live view: every one of these is built from an array the
  // caller has already read out of the bridge, and re-reading the tree per index
  // would be a bridge call per element of every loop over one.
  const defineIndexed = (target, items) => {
    Object.defineProperty(target, "length", { value: items.length, enumerable: false });
    defineMembers(target, { ...items });
    return Object.freeze(target);
  };
  // Interface constants: the names an interface numbers from zero, put on both
  // the constructor and the prototype because both spellings are read —
  // `WebSocket.OPEN` and `socket.OPEN` are the same constant.
  const defineConstants = (constructor, names) => {
    const values = Object.fromEntries(names.map((name, value) => [name, value]));
    for (const target of [constructor, constructor.prototype]) defineMembers(target, values);
  };
  const bridgeCallCounts = testHarness ? new Map() : null;
  const rawCall = (operation, ...args) =>
    JSON.parse(__blitsenDomCall(operation, ...args.map(value => String(value))));
  const call = testHarness
    ? (operation, ...args) => {
      bridgeCallCounts.set(operation, (bridgeCallCounts.get(operation) ?? 0) + 1);
      return rawCall(operation, ...args);
    }
    : rawCall;
  const handle = Symbol("Blitsen node handle");
  let nextAnimationFrameId = 1;
  let animationFrames = new Map();
  let runningAnimationFrames = null;
  let forcedLayoutsThisFrame = 0;
  const recordForcedLayout = result => {
    if (result.forced) forcedLayoutsThisFrame++;
    return result;
  };
  // The shape every geometry read answers in: the box, its four edges, and a
  // `toJSON` that carries both. Frozen, because a client rectangle is a reading
  // taken at a moment and not a handle onto the box it measured.
  const clientRect = (x, y, width, height) => {
    const values = { x, y, width, height,
      top: y, right: x + width, bottom: y + height, left: x };
    return Object.freeze({ ...values, toJSON() { return { ...values }; } });
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
    // The frame's timestamp is also the clock the cascade samples animations and
    // transitions at, and it is set before anything else runs: a callback that
    // forces layout must see the frame it is in, not the one before it. Nothing
    // below reads a clock of its own, so a replayed trace animates identically.
    call("setAnimationTime", Number(timestamp));
    notifySurfaceResizes();
    notifyResizeObservers();
    notifyMediaQueries();
    settleFetches();
    settleSockets();
    settleEventSources();
    settlePorts();
    settleAudio();
    settleImages();
    settleLinks();
    deliverSecondInstances();
    settleDialogs();
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
    // In-flight requests, live sockets and event streams, undecoded images,
    // unfetched stylesheets and undelivered resize observations keep the host
    // turning: their landing point is this function, so a loop that stopped
    // would never deliver them.
    // A running CSS animation is owed a frame for the same reason — the clock
    // only moves when this is called, so a loop that idled would freeze it
    // part-way through. An open dialog is the same argument twice over: its
    // answer lands here, and the window has to go on painting behind it rather
    // than freeze until it is dismissed.
    return animationFrames.size + inflightFetches.size + liveSockets.size
      + liveEventSources.size
      + pendingResizeObservations() + waitingImages() + waitingLinks()
      + (audioPending() ? 1 : 0)
      + (call("isAnimating") ? 1 : 0) + (nativeDialogPending() ? 1 : 0)
      + (portsPending() ? 1 : 0);
  };

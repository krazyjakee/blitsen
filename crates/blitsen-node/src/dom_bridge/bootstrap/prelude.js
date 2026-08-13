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
    settleAudio();
    settleImages();
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
    // In-flight requests, live sockets, undecoded images and undelivered resize
    // observations keep the host turning: their landing point is this function,
    // so a loop that stopped would never deliver them. A running CSS animation
    // is owed a frame for the same reason — the clock only moves when this is
    // called, so a loop that idled would freeze it part-way through. An open
    // dialog is the same argument twice over: its answer lands here, and the
    // window has to go on painting behind it rather than freeze until it is
    // dismissed.
    return animationFrames.size + inflightFetches.size + liveSockets.size
      + pendingResizeObservations() + waitingImages() + (audioPending() ? 1 : 0)
      + (call("isAnimating") ? 1 : 0) + (nativeDialogPending() ? 1 : 0);
  };


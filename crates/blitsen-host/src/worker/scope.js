  // The dedicated worker global scope.
  //
  // `self` is the global object, messages arrive on it rather than on a port the
  // application can see, and `postMessage` speaks to whoever constructed this
  // worker. The port underneath is started immediately: a worker that had to ask
  // for its own messages would drop everything posted before its script finished
  // loading, and the whole point of the queue is that it does not.
  const scope = new EventTarget();
  const selfPort = new MessagePort(__blitsenWorkerSelfPort());
  portState(selfPort).target = scope;
  selfPort.start();

  const location = Object.freeze({
    ...resolveAgainstDocument(workerUrl),
    toString: () => workerUrl,
  });

  const install = (name, value) =>
    Object.defineProperty(globalThis, name, {
      value, writable: true, enumerable: false, configurable: true,
    });
  const accessor = (name, get, set) =>
    Object.defineProperty(globalThis, name, { get, set, configurable: true });

  for (const method of ["addEventListener", "removeEventListener", "dispatchEvent"])
    install(method, EventTarget.prototype[method].bind(scope));

  install("self", globalThis);
  install("location", location);
  // Identity and no capability, exactly as the document's answers. A worker has
  // a `navigator` in a browser, and code that runs in both looks for it to
  // decide which it is in.
  install("navigator", Object.freeze(JSON.parse(__blitsenNavigatorState)));
  install("name", workerIdentity.name);
  install("Event", Event);
  install("EventTarget", EventTarget);
  install("MessageEvent", MessageEvent);
  install("ErrorEvent", ErrorEvent);
  install("MessagePort", MessagePort);
  install("MessageChannel", MessageChannel);
  install("Worker", Worker);
  install("structuredClone", structuredClone);
  install("Headers", Headers);
  install("Request", Request);
  install("Response", Response);
  install("Blob", Blob);
  install("AbortController", AbortController);
  install("AbortSignal", AbortSignal);
  install("fetch", fetch);
  install("postMessage", (message, options = []) => selfPort.postMessage(message, options));
  // Ends the worker after the turn it was called in finishes, which is what the
  // specification asks for: the queue is discarded rather than drained, but the
  // callback that called `close` runs to its end.
  install("close", () => __blitsenWorkerStop());
  accessor("onmessage", () => portHandlers.get(scope)?.message ?? null,
    callback => setPortHandler(scope, "message", callback));
  accessor("onmessageerror", () => portHandlers.get(scope)?.messageerror ?? null,
    callback => setPortHandler(scope, "messageerror", callback));

  // Everything uncaught on this thread goes through `reportError` — a listener
  // that threw, a rejected module evaluation — so it is the one place that has
  // to tell the other side. Still written to this process's stderr as well:
  // an application with no `onerror` should not lose the message entirely.
  const reportLocally = globalThis.reportError;
  install("reportError", error => {
    reportLocally(error);
    const detail = error instanceof Error && error.stack
      ? `${error}\n${error.stack}` : String(error);
    __blitsenWorkerFailed(detail);
  });

  // The worker's landing point, called once per turn by its own loop. The
  // returned count is what tells that loop whether anything is still owed —
  // an outstanding `fetch` has no waker of its own, so a worker waiting on one
  // has to be told to look again rather than parking until a message arrives.
  install("__blitsenWorkerTurn", () => {
    settlePorts();
    settleFetches();
    return inflightFetches.size;
  });

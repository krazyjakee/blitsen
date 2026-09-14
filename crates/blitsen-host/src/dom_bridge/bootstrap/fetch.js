  // Bun owns HTTP, bodies, streams and cancellation. This adapter only resolves
  // application URLs and delivers the initial result on the document frame.
  const inflightFetches = new Map();
  let nextFetch = 0;
  let documentFetchController = new host.AbortController();
  const fetch = (input, init) => {
    const id = ++nextFetch;
    if (disposed) return Promise.reject(new DOMException("Document closed", "AbortError"));
    const controller = documentFetchController;
    const callerSignal = init?.signal ?? (input instanceof host.Request ? input.signal : undefined);
    const signal = callerSignal
      ? host.AbortSignal.any([callerSignal, controller.signal]) : controller.signal;
    return new Promise((resolve, reject) => {
      inflightFetches.set(id, { reject });
      const complete = callback => queueHostCompletion(() => {
        if (!inflightFetches.delete(id)) return;
        callback();
      });
      try {
        const request = input instanceof host.Request
          ? new host.Request(runtimeUrl(input.url), input)
          : runtimeUrl(input);
        host.fetch(request, { ...init, signal }).then(
          response => complete(() => resolve(response)),
          error => complete(() => reject(error)));
      } catch (error) { complete(() => reject(error)); }
    });
  };
  const settleFetches = settleHostCompletions;
  const stop = () => {
    documentFetchController.abort();
    documentFetchController = new host.AbortController();
  };

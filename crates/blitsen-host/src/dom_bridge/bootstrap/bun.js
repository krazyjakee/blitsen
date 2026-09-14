  // Capture Bun's primitives once, before installing the document facade. A
  // reload shares the host realm and must never capture the previous facade.
  const hostKey = Symbol.for("blitsen.bun.primitives");
  const host = globalThis[hostKey] ??= Object.freeze({
    fetch: globalThis.fetch.bind(globalThis),
    Headers: globalThis.Headers, Request: globalThis.Request, Response: globalThis.Response, Blob: globalThis.Blob, File: globalThis.File, FormData: globalThis.FormData, AbortController: globalThis.AbortController, AbortSignal: globalThis.AbortSignal,
    URL: globalThis.URL, URLSearchParams: globalThis.URLSearchParams, TextEncoder: globalThis.TextEncoder, TextDecoder: globalThis.TextDecoder, Intl: globalThis.Intl,
    WebSocket: globalThis.WebSocket, Worker: globalThis.Worker, MessageChannel: globalThis.MessageChannel, MessagePort: globalThis.MessagePort,
    structuredClone: globalThis.structuredClone.bind(globalThis),
    setTimeout: globalThis.setTimeout.bind(globalThis),
    clearTimeout: globalThis.clearTimeout.bind(globalThis),
    setInterval: globalThis.setInterval.bind(globalThis),
    clearInterval: globalThis.clearInterval.bind(globalThis),
  });
  const Headers = host.Headers, Request = host.Request, Response = host.Response;
  const Blob = host.Blob, File = host.File, FormData = host.FormData;
  const AbortController = host.AbortController, AbortSignal = host.AbortSignal;
  const URL = host.URL, URLSearchParams = host.URLSearchParams, Intl = host.Intl;
  const MessageChannel = host.MessageChannel, MessagePort = host.MessagePort;
  const structuredClone = host.structuredClone;
  const runtimeRoot = globalThis.__blitsenRuntimeRoot;
  delete globalThis.__blitsenRuntimeRoot;
  const runtimeUrl = value => {
    const url = new host.URL(String(value), currentUrl());
    if (url.protocol === "blitsen:" && url.hostname === "app") {
      if (!runtimeRoot) throw new TypeError("This document has no application files");
      const file = new host.URL(url.pathname.replace(/^\//, ""), runtimeRoot);
      file.search = url.search;
      file.hash = url.hash;
      return file.href;
    }
    return url.href;
  };
  // Bun delivers asynchronous work between native calls. Queue document events
  // until the next frame, where all other native completions are dispatched.
  let disposed = false;
  const hostCompletions = [];
  const queueHostCompletion = callback => { if (!disposed) hostCompletions.push(callback); };
  const settleHostCompletions = () => {
    for (const callback of hostCompletions.splice(0)) {
      if (disposed) break;
      try { callback(); } catch (error) { reportError(error); }
    }
  };
  if (runtimeRoot?.startsWith("file:")) {
    const { fileURLToPath } = process.getBuiltinModule("url");
    const prefix = fileURLToPath(runtimeRoot);
    const require = process.getBuiltinModule("module").createRequire(prefix + "index.js");
    for (const key of Object.keys(require.cache)) if (key.startsWith(prefix)) delete require.cache[key];
  }

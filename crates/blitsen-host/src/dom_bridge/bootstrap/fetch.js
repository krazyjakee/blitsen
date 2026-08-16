  // Networking. Blitsen's own fetch rather than the host's, so the Phase 2
  // engine swap is invisible to the application. There is no same-origin policy
  // and no CORS: an exported application is trusted native software, not a
  // document. Bodies are buffered — see COMPATIBILITY.md for why streaming is
  // not in this tier.
  const HEADER_NAME = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  const headerFields = new WeakMap();
  const fieldsFor = headers => {
    const fields = headerFields.get(headers);
    if (!fields) throw new TypeError("Illegal invocation");
    return fields;
  };

  class Headers {
    constructor(init) {
      headerFields.set(this, new Map());
      if (init === undefined || init === null) return;
      if (init instanceof Headers || Array.isArray(init)) {
        for (const pair of init) {
          if (!Array.isArray(pair) || pair.length !== 2)
            throw new TypeError("Headers entries must be [name, value] pairs");
          this.append(pair[0], pair[1]);
        }
        return;
      }
      if (typeof init !== "object") throw new TypeError("invalid Headers initializer");
      for (const name of Object.keys(init)) this.append(name, init[name]);
    }
    _name(name) {
      const key = String(name).toLowerCase();
      if (!HEADER_NAME.test(key)) throw new TypeError(`invalid header name: ${name}`);
      return key;
    }
    append(name, value) {
      const key = this._name(name);
      const fields = fieldsFor(this);
      const next = String(value).trim();
      const existing = fields.get(key);
      fields.set(key, existing === undefined ? next : `${existing}, ${next}`);
    }
    set(name, value) { fieldsFor(this).set(this._name(name), String(value).trim()); }
    get(name) { return fieldsFor(this).get(this._name(name)) ?? null; }
    has(name) { return fieldsFor(this).has(this._name(name)); }
    delete(name) { fieldsFor(this).delete(this._name(name)); }
    forEach(callback, thisArg) { for (const [name, value] of this) callback.call(thisArg, value, name, this); }
    *entries() {
      const fields = fieldsFor(this);
      for (const name of [...fields.keys()].sort()) yield [name, fields.get(name)];
    }
    *keys() { for (const [name] of this) yield name; }
    *values() { for (const [, value] of this) yield value; }
    [Symbol.iterator]() { return this.entries(); }
  }

  const blobBytes = new WeakMap();
  const bytesOf = blob => {
    const bytes = blobBytes.get(blob);
    if (!bytes) throw new TypeError("Illegal invocation");
    return bytes;
  };
  const concatBytes = chunks => {
    const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    return bytes;
  };
  const asBytes = value => {
    if (typeof value === "string") return __blitsenUtf8Encode(value);
    if (value instanceof Blob) return bytesOf(value);
    if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength));
    if (value instanceof ArrayBuffer) return new Uint8Array(value.slice(0));
    return null;
  };

  class Blob {
    constructor(parts = [], options = {}) {
      blobBytes.set(this, concatBytes([...parts].map(part => asBytes(part) ?? __blitsenUtf8Encode(String(part)))));
      defineMembers(this, { type: String(options.type ?? "").toLowerCase() });
    }
    get size() { return bytesOf(this).length; }
    slice(start, end, type) { return new Blob([bytesOf(this).slice(start, end)], { type: type ?? "" }); }
    text() { return Promise.resolve(__blitsenUtf8Decode(bytesOf(this))); }
    arrayBuffer() { return Promise.resolve(bytesOf(this).slice().buffer); }
  }

  const signalStates = new WeakMap();
  const signalState = signal => {
    const state = signalStates.get(signal);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };
  const createSignal = () => {
    const signal = Object.create(AbortSignal.prototype);
    signalStates.set(signal, { aborted: false, reason: undefined, onabort: null });
    return signal;
  };
  const raiseAbort = (signal, reason) => {
    const state = signalState(signal);
    if (state.aborted) return;
    state.aborted = true;
    state.reason = reason ?? new DOMException("The operation was aborted", "AbortError");
    signal.dispatchEvent(new Event("abort"));
  };

  class AbortSignal extends EventTarget {
    constructor() { throw new TypeError("Illegal constructor"); }
    get aborted() { return signalState(this).aborted; }
    get reason() { return signalState(this).reason; }
    get onabort() { return signalState(this).onabort; }
    set onabort(callback) {
      const state = signalState(this);
      setEventHandler(this, state, "abort", callback, "onabort");
    }
    throwIfAborted() { const state = signalState(this); if (state.aborted) throw state.reason; }
    static abort(reason) { const signal = createSignal(); raiseAbort(signal, reason); return signal; }
    static timeout(milliseconds) {
      const signal = createSignal();
      setTimeout(() => raiseAbort(signal, new DOMException("The operation timed out", "TimeoutError")),
        Number(milliseconds));
      return signal;
    }
  }

  class AbortController {
    constructor() { defineMembers(this, { signal: createSignal() }); }
    abort(reason) { raiseAbort(this.signal, reason); }
  }

  const bodyStates = new WeakMap();
  const bodyStateFor = target => {
    const state = bodyStates.get(target);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };
  // A body the application never reads still occupies Rust memory, and only the
  // collector knows the Response was abandoned.
  const abandonedBodies = new FinalizationRegistry(id => __blitsenFetchCancel(String(id)));
  const readBody = (target, kind) => {
    const state = bodyStateFor(target);
    if (state.used) return Promise.reject(new TypeError("the body has already been read"));
    state.used = true;
    try {
      if (state.id === null) {
        const bytes = state.bytes ?? new Uint8Array(0);
        return Promise.resolve(kind === "text" ? __blitsenUtf8Decode(bytes) : bytes);
      }
      const id = state.id;
      state.id = null;
      abandonedBodies.unregister(target);
      return Promise.resolve(__blitsenFetchBody(String(id), kind));
    } catch (error) {
      return Promise.reject(error);
    }
  };
  const installBodyMethods = prototype => Object.defineProperties(prototype, {
    bodyUsed: { get() { return bodyStateFor(this).used; } },
    text: { value() { return readBody(this, "text"); } },
    json: { value() { return readBody(this, "text").then(text => JSON.parse(text)); } },
    arrayBuffer: { value() { return readBody(this, "bytes").then(bytes => bytes.buffer); } },
    blob: { value() {
      return readBody(this, "bytes").then(bytes => new Blob([bytes], { type: this.headers.get("content-type") ?? "" }));
    } },
  });

  const KNOWN_METHODS = ["DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"];
  const normalizeMethod = method => {
    const value = String(method);
    const upper = value.toUpperCase();
    return KNOWN_METHODS.includes(upper) ? upper : value;
  };
  const encodeBody = (body, headers) => {
    if (body === undefined || body === null) return null;
    const bytes = asBytes(body);
    if (bytes === null)
      throw new TypeError("a fetch body must be a string, Blob, ArrayBuffer, or typed array");
    if (!headers.has("content-type")) {
      if (typeof body === "string") headers.set("content-type", "text/plain;charset=UTF-8");
      else if (body instanceof Blob && body.type) headers.set("content-type", body.type);
    }
    return body instanceof Blob ? bytes.slice() : bytes;
  };

  class Request {
    constructor(input, options = {}) {
      const source = input instanceof Request ? input : null;
      const headers = new Headers(options.headers ?? source?.headers);
      const method = normalizeMethod(options.method ?? source?.method ?? "GET");
      const body = "body" in options
        ? encodeBody(options.body, headers)
        : source ? bodyStateFor(source).bytes : null;
      if (body !== null && (method === "GET" || method === "HEAD"))
        throw new TypeError(`a ${method} request cannot have a body`);
      const signal = options.signal ?? source?.signal ?? null;
      if (signal !== null && !(signal instanceof AbortSignal))
        throw new TypeError("fetch signal must be an AbortSignal");
      defineMembers(this, {
        method,
        url: resolveAgainstDocument(source ? source.url : String(input)).href,
        headers,
        signal,
      });
      bodyStates.set(this, { used: false, id: null, bytes: body });
    }
  }
  installBodyMethods(Request.prototype);

  class Response {
    constructor(body = null, options = {}) {
      const status = options.status === undefined ? 200 : Number(options.status);
      if (!Number.isInteger(status) || status < 200 || status > 599)
        throw new RangeError(`invalid response status: ${options.status}`);
      const headers = new Headers(options.headers);
      defineMembers(this, {
        status,
        statusText: String(options.statusText ?? ""),
        headers,
        ok: status >= 200 && status < 300,
        url: "",
        redirected: false,
      });
      bodyStates.set(this, { used: false, id: null, bytes: encodeBody(body, headers) });
    }
    static json(data, options = {}) {
      const response = new Response(JSON.stringify(data), options);
      response.headers.set("content-type", "application/json");
      return response;
    }
  }
  installBodyMethods(Response.prototype);

  const receivedResponse = record => {
    const response = Object.create(Response.prototype);
    defineMembers(response, {
      status: record.status,
      statusText: record.statusText,
      headers: new Headers(record.headers),
      ok: record.ok,
      url: record.url,
      redirected: record.redirected,
    });
    bodyStates.set(response, { used: false, id: record.id, bytes: null });
    abandonedBodies.register(response, record.id, response);
    return response;
  };

  const inflightFetches = new Map();
  const fetchFailure = error => error.name === "TypeError"
    ? new TypeError(error.message)
    : new DOMException(error.message, error.name);
  // The one handoff point for network work: completions become settled promises
  // here, before any requestAnimationFrame callback of the same turn runs.
  const settleFetches = () => {
    if (inflightFetches.size === 0) return;
    for (const record of JSON.parse(__blitsenFetchPoll()).completed) {
      const pending = inflightFetches.get(record.id);
      if (!pending) { __blitsenFetchCancel(String(record.id)); continue; }
      inflightFetches.delete(record.id);
      pending.detach();
      if (record.error) pending.reject(fetchFailure(record.error));
      else pending.resolve(receivedResponse(record));
    }
  };

  const fetch = (input, options = {}) => {
    let request;
    let id;
    try {
      request = new Request(input, options);
      if (request.signal?.aborted) return Promise.reject(signalState(request.signal).reason);
      const state = bodyStateFor(request);
      state.used = true;
      id = __blitsenFetchStart(JSON.stringify({
        url: request.url, method: request.method, headers: [...request.headers],
      }), state.bytes);
    } catch (error) {
      return Promise.reject(error);
    }
    return new Promise((resolve, reject) => {
      const signal = request.signal;
      const onAbort = signal && (() => {
        inflightFetches.delete(id);
        __blitsenFetchCancel(String(id));
        reject(signalState(signal).reason);
      });
      inflightFetches.set(id, {
        resolve, reject,
        detach: () => { if (onAbort) signal.removeEventListener("abort", onAbort); },
      });
      if (onAbort) signal.addEventListener("abort", onAbort, { once: true });
    });
  };

  // `window.stop()`: abort the document's in-flight loading. Every outstanding
  // `fetch` is rejected the way its own AbortSignal would reject it, and every
  // subresource the renderer is still waiting on is cancelled and settled — a
  // request left pending would block painting rather than end the load.
  //
  // Timers and animation frames are left running, because a browser leaves them
  // running: they are the application's own work, not the document's load. Nor
  // is there a parser to stop; a Blitsen document is parsed whole before any
  // script of it runs. With nothing in flight both halves still run and find
  // nothing, which is a no-op in effect rather than a no-op implementation.
  const stop = () => {
    for (const [id, pending] of inflightFetches) {
      inflightFetches.delete(id);
      pending.detach();
      __blitsenFetchCancel(String(id));
      pending.reject(new DOMException("The operation was aborted", "AbortError"));
    }
    call("stopLoading");
  };

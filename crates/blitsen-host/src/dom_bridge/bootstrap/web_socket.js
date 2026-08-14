  // WebSocket. The connection runs on the same worker pool `fetch` does and is
  // delivered at the same point in the frame turn: the handshake, every frame
  // and the close land at the start of the animation-frame stage, so an
  // application is never re-entered from a socket thread part-way through one.
  //
  // Binary payloads stay in Rust until this drains them, so the bytes cross the
  // boundary once and in the shape `binaryType` asked for.
  const socketStates = new WeakMap();
  const socketState = socket => {
    const state = socketStates.get(socket);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };
  // A connecting, open or closing socket keeps the host turning: its events land
  // in `animationFrameTick`, so a loop that idled would never deliver them. It
  // is dropped once its `close` has been delivered and nothing more is owed.
  const liveSockets = new Map();
  // A subprotocol is a header token, and a browser refuses one that is not
  // before it opens a connection rather than sending a handshake that cannot be
  // answered.
  const PROTOCOL_TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  const socketHandlers = new WeakMap();
  const setSocketHandler = (socket, type, callback) => {
    let handlers = socketHandlers.get(socket);
    if (!handlers) socketHandlers.set(socket, handlers = {});
    if (handlers[type]) socket.removeEventListener(type, handlers[type]);
    handlers[type] = typeof callback === "function" ? callback : null;
    if (handlers[type]) socket.addEventListener(type, handlers[type]);
  };
  const socketMessage = (state, record) => {
    if (record.text !== undefined) return record.text;
    const bytes = __blitsenSocketBinary(String(state.id), String(record.binary));
    return state.binaryType === "blob" ? new Blob([bytes]) : bytes.buffer;
  };
  // The one handoff point for socket work, for the same reason `fetch` has one.
  const settleSockets = () => {
    if (liveSockets.size === 0) return;
    for (const record of JSON.parse(__blitsenSocketPoll())) {
      const socket = liveSockets.get(record.id);
      if (!socket) continue;
      const state = socketState(socket);
      let event;
      if (record.type === "message") {
        event = new MessageEvent("message",
          { data: socketMessage(state, record), origin: state.origin });
      } else if (record.type === "close") {
        state.readyState = 3;
        liveSockets.delete(record.id);
        event = new CloseEvent("close",
          { code: record.code, reason: record.reason, wasClean: record.wasClean });
      } else {
        if (record.type === "open") {
          state.readyState = 1;
          state.protocol = record.protocol;
        }
        // A browser's error event carries no detail and neither does this one,
        // but a handshake that failed for a nameable reason says so where a
        // developer will see it rather than leaving a silent close behind.
        if (record.message) console.error(`WebSocket connection to ${state.url} failed: ${record.message}`);
        event = new Event(record.type);
      }
      socket.dispatchEvent(event);
    }
  };

  class WebSocket extends EventTarget {
    constructor(url, protocols = []) {
      super();
      const requested = protocols === undefined || protocols === null ? []
        : Array.isArray(protocols) ? protocols.map(String) : [String(protocols)];
      for (const protocol of requested)
        if (!PROTOCOL_TOKEN.test(protocol))
          throw new DOMException(`"${protocol}" is not a subprotocol token`, "SyntaxError");
      if (new Set(requested).size !== requested.length)
        throw new DOMException("the same subprotocol was requested twice", "SyntaxError");
      const target = resolveAgainstDocument(url);
      if (target.protocol !== "ws:" && target.protocol !== "wss:")
        throw new DOMException(`${target.href} is not a ws: or wss: address`, "SyntaxError");
      if (target.hash)
        throw new DOMException("a WebSocket address cannot carry a fragment", "SyntaxError");
      const id = __blitsenSocketOpen(target.href, JSON.stringify(requested));
      socketStates.set(this, { id, readyState: 0, protocol: "", binaryType: "blob",
        url: target.href, origin: target.origin });
      liveSockets.set(id, this);
    }
    get url() { return socketState(this).url; }
    get readyState() { return socketState(this).readyState; }
    get protocol() { return socketState(this).protocol; }
    // Nothing is ever negotiated: no extension is offered in the handshake, so
    // the truthful answer is the empty string rather than an absent property.
    get extensions() { return ""; }
    get bufferedAmount() { return __blitsenSocketBuffered(String(socketState(this).id)); }
    get binaryType() { return socketState(this).binaryType; }
    set binaryType(value) {
      const kind = String(value);
      if (kind !== "blob" && kind !== "arraybuffer")
        throw new TypeError(`binaryType is "blob" or "arraybuffer", not "${kind}"`);
      socketState(this).binaryType = kind;
    }
    send(data) {
      const state = socketState(this);
      if (state.readyState === 0)
        throw new DOMException("the connection is still opening", "InvalidStateError");
      // Discarded rather than refused once the socket is closing or closed,
      // which is what the spec says and what a reconnecting client relies on.
      if (state.readyState !== 1) return;
      if (typeof data === "string") { __blitsenSocketSendText(String(state.id), data); return; }
      const bytes = asBytes(data);
      if (bytes === null)
        throw new TypeError("a WebSocket message must be a string, Blob, ArrayBuffer, or typed array");
      __blitsenSocketSendBinary(String(state.id), bytes);
    }
    close(code, reason = "") {
      const state = socketState(this);
      const status = code === undefined || code === null ? null : Math.trunc(Number(code));
      if (status !== null && status !== 1000 && !(status >= 3000 && status <= 4999))
        throw new DOMException(`${code} is not a close code an application may send`, "InvalidAccessError");
      const text = String(reason);
      if (__blitsenUtf8Encode(text).length > 123)
        throw new DOMException("a close reason is at most 123 bytes of UTF-8", "SyntaxError");
      if (state.readyState > 1) return;
      state.readyState = 2;
      __blitsenSocketClose(String(state.id), status === null ? "" : String(status), text);
    }
    get onopen() { return socketHandlers.get(this)?.open ?? null; }
    set onopen(callback) { setSocketHandler(this, "open", callback); }
    get onmessage() { return socketHandlers.get(this)?.message ?? null; }
    set onmessage(callback) { setSocketHandler(this, "message", callback); }
    get onerror() { return socketHandlers.get(this)?.error ?? null; }
    set onerror(callback) { setSocketHandler(this, "error", callback); }
    get onclose() { return socketHandlers.get(this)?.close ?? null; }
    set onclose(callback) { setSocketHandler(this, "close", callback); }
  }
  defineConstants(WebSocket, ["CONNECTING", "OPEN", "CLOSING", "CLOSED"]);


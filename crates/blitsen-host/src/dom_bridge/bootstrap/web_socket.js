  // The transport, protocol validation and buffering belong to Bun. Blitsen
  // only associates the socket with a document and queues its public events.
  const liveSockets = new Set();
  const socketStates = new WeakMap();
  const socketHandlers = new WeakMap();
  const setSocketHandler = (socket, type, callback) => {
    let handlers = socketHandlers.get(socket);
    if (!handlers) socketHandlers.set(socket, handlers = {});
    setEventHandler(socket, handlers, type, callback);
  };
  class WebSocket extends EventTarget {
    constructor(url, protocols) {
      super();
      const socket = new host.WebSocket(runtimeUrl(url), protocols);
      socketStates.set(this, socket);
      liveSockets.add(this);
      for (const type of ["open", "message", "error", "close"]) {
        socket.addEventListener(type, event => queueHostCompletion(() => {
          if (!liveSockets.has(this)) return;
          if (type === "close") liveSockets.delete(this);
          this.dispatchEvent(type === "message"
            ? new MessageEvent(type, { data: event.data, origin: event.origin })
            : type === "close" ? new CloseEvent(type, event) : new Event(type));
        }));
      }
    }
    get url() { return socketStates.get(this).url; }
    get readyState() { return socketStates.get(this).readyState; }
    get protocol() { return socketStates.get(this).protocol; }
    get extensions() { return socketStates.get(this).extensions; }
    get bufferedAmount() { return socketStates.get(this).bufferedAmount; }
    get binaryType() { return socketStates.get(this).binaryType; }
    set binaryType(value) { socketStates.get(this).binaryType = value; }
    send(data) { socketStates.get(this).send(data); }
    close(code, reason) { socketStates.get(this).close(code, reason); }
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
  const settleSockets = () => {};

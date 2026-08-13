  // Message ports, channels and workers.
  //
  // Loaded into both global scopes — the document's and a worker's — because
  // both own ports and both send the same messages through them. What differs
  // between the two is supplied around this fragment: the document resolves a
  // worker URL against its own address, a worker resolves against its script's,
  // and the bare global `postMessage` means different things in each.
  //
  // Delivery is polled, never pushed. The host queues what arrives on a port and
  // this drains it at one point in the turn — the start of the animation-frame
  // stage in the document, the top of the loop in a worker — which is the same
  // contract `fetch` completions and socket frames are delivered under, and the
  // reason a message cannot arrive part-way through a callback.
  // Carries what a worker threw where nothing could catch it. `filename` and
  // `lineno` are present and empty: the engines behind this do not report a
  // position for an exception that escaped a module's evaluation, and a zero is
  // a truthful "not known" where an absent property would break a handler that
  // reads it unguarded.
  class ErrorEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperties(this, {
        message: { value: String(options.message ?? ""), enumerable: true },
        filename: { value: String(options.filename ?? ""), enumerable: true },
        lineno: { value: Number(options.lineno ?? 0), enumerable: true },
        colno: { value: Number(options.colno ?? 0), enumerable: true },
        error: { value: options.error ?? null, enumerable: true },
      });
    }
  }

  const portStates = new WeakMap();
  const livePorts = new Map();
  const portState = port => {
    const state = portStates.get(port);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };
  const portHandlers = new WeakMap();
  const setPortHandler = (target, type, callback) => {
    let handlers = portHandlers.get(target);
    if (!handlers) portHandlers.set(target, handlers = {});
    if (handlers[type]) target.removeEventListener(type, handlers[type]);
    handlers[type] = typeof callback === "function" ? callback : null;
    if (handlers[type]) target.addEventListener(type, handlers[type]);
  };

  class MessagePort extends EventTarget {
    // Never constructed by an application: a port only exists as one end of a
    // pair, which is what `MessageChannel` and `new Worker` hand out.
    constructor(id) {
      super();
      if (typeof id !== "number") throw new TypeError("Illegal constructor");
      portStates.set(this, { id, started: false, detached: false, target: this });
      livePorts.set(id, this);
    }
    postMessage(message, options = []) {
      const state = portState(this);
      if (state.detached) return;
      const transfer = Array.isArray(options) ? options : (options?.transfer ?? []);
      const encoded = encodeClone(message, transfer);
      stageBuffers(encoded.buffers);
      __blitsenPortPost(String(state.id), encoded.graph, JSON.stringify(encoded.ports));
    }
    // Delivery is off until it is asked for, which is what lets a port be handed
    // on to somewhere else without losing the messages queued for it.
    start() {
      const state = portState(this);
      if (state.started || state.detached) return;
      state.started = true;
      __blitsenPortStart(String(state.id));
    }
    close() {
      const state = portState(this);
      if (state.detached) return;
      state.detached = true;
      livePorts.delete(state.id);
      __blitsenPortClose(String(state.id));
    }
    get onmessage() { return portHandlers.get(this)?.message ?? null; }
    // Assigning `onmessage` starts the port, as the specification says: an
    // application that never calls `start()` still receives its messages.
    set onmessage(callback) {
      setPortHandler(this, "message", callback);
      this.start();
    }
    get onmessageerror() { return portHandlers.get(this)?.messageerror ?? null; }
    set onmessageerror(callback) { setPortHandler(this, "messageerror", callback); }
  }

  class MessageChannel {
    constructor() {
      const [first, second] = JSON.parse(__blitsenPortChannel());
      Object.defineProperties(this, {
        port1: { value: new MessagePort(first), enumerable: true },
        port2: { value: new MessagePort(second), enumerable: true },
      });
    }
  }

  // Ports arriving with a message. They are already this context's as far as the
  // host is concerned — ownership moved when the message was queued — so this
  // only gives each one an object to be reached through. Memoized per delivery,
  // because a port named both in the transfer list and inside the message is one
  // port, and an application comparing `event.ports[0]` with what it found in
  // `event.data` must be comparing the same object.
  const portAdopter = () => {
    const adopted = new Map();
    return id => {
      const key = Number(id);
      if (!adopted.has(key)) adopted.set(key, new MessagePort(key));
      return adopted.get(key);
    };
  };
  const portIdOf = port => portState(port).id;
  // Handing a port on detaches it here: the object stays, and does nothing.
  const detachPort = port => {
    const state = portState(port);
    if (state.detached) throw new DOMException("this port has already been transferred.", "DataCloneError");
    state.detached = true;
    livePorts.delete(state.id);
    return state.id;
  };

  const decodeDelivered = record => {
    const adopt = portAdopter();
    const data = decodeClone(record.data, takeBuffers(record.buffers), adopt);
    return { data, ports: record.ports.map(adopt) };
  };

  // The one handoff point for messaging, for the same reason `fetch` has one.
  const settlePorts = () => {
    if (livePorts.size === 0) return;
    for (const record of JSON.parse(__blitsenPortPoll())) {
      const port = livePorts.get(record.port);
      if (!port) continue;
      const target = portState(port).target;
      if (record.type === "message") {
        let decoded;
        try {
          decoded = decodeDelivered(record);
        } catch (error) {
          // What the web does with a message it received and could not
          // deserialize: the failure is an event, not an exception thrown at
          // whatever happened to be running.
          console.error("A message could not be deserialized", error);
          target.dispatchEvent(new MessageEvent("messageerror", { origin: messageOrigin }));
          continue;
        }
        target.dispatchEvent(new MessageEvent("message",
          { data: decoded.data, ports: decoded.ports, origin: messageOrigin }));
      } else if (record.type === "error") {
        // An exception nothing in the worker caught. Reported here rather than
        // only on its own thread, because the application that started the
        // worker is the one that can do something about it.
        target.dispatchEvent(new ErrorEvent("error", { message: record.message }));
      } else if (record.type === "close") {
        portState(port).detached = true;
        livePorts.delete(record.port);
      }
    }
  };
  const portsPending = () => livePorts.size > 0 && __blitsenPortPending();

  const workerStates = new WeakMap();
  const workerState = worker => {
    const state = workerStates.get(worker);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };
  const liveWorkers = new Map();

  class Worker extends EventTarget {
    constructor(url, options = {}) {
      super();
      const script = resolveWorkerUrl(url);
      const type = options?.type === "module" ? "module" : "classic";
      const started = JSON.parse(
        __blitsenWorkerStart(script, type, String(options?.name ?? "")));
      const port = new MessagePort(started.port);
      // The pair behind a dedicated worker is not the application's to see: its
      // messages arrive on the Worker object itself. So the port dispatches
      // there and is started at once, rather than waiting for an `onmessage`
      // that will never be set on it.
      portState(port).target = this;
      port.start();
      workerStates.set(this, { id: started.worker, port });
      liveWorkers.set(started.worker, this);
    }
    postMessage(message, options = []) { workerState(this).port.postMessage(message, options); }
    // Nothing is owed to a terminated worker: whatever it had queued for this
    // side is dropped, which is what the specification asks for and what an
    // application replacing one worker with another relies on.
    terminate() {
      const state = workerState(this);
      if (!liveWorkers.delete(state.id)) return;
      __blitsenWorkerTerminate(String(state.id));
      state.port.close();
    }
    get onmessage() { return portHandlers.get(this)?.message ?? null; }
    set onmessage(callback) { setPortHandler(this, "message", callback); }
    get onmessageerror() { return portHandlers.get(this)?.messageerror ?? null; }
    set onmessageerror(callback) { setPortHandler(this, "messageerror", callback); }
    get onerror() { return portHandlers.get(this)?.error ?? null; }
    set onerror(callback) { setPortHandler(this, "error", callback); }
  }

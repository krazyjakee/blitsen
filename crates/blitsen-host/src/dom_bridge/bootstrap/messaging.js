  class ErrorEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, {
        message: String(options.message ?? ""),
        filename: String(options.filename ?? ""),
        lineno: Number(options.lineno ?? 0),
        colno: Number(options.colno ?? 0),
        error: options.error ?? null,
      });
    }
  }

  // Every message that crosses a boundary arrives as one of these, and the
  // boundaries are what this fragment is: a port, a channel, a worker, and — in
  // the document — a socket. It lives here rather than beside the DOM's other
  // events because a message has to mean the same thing at both ends of a port,
  // and the two ends are in different scopes running different bootstraps.
  //
  // The members a given sender cannot fill are present and empty rather than
  // absent: `source` and `ports` are truthfully nothing when the message came
  // off a socket, and a library reads them unguarded.
  class MessageEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, {
        data: options.data ?? null,
        origin: String(options.origin ?? ""),
        lastEventId: String(options.lastEventId ?? ""),
        source: options.source ?? null,
        ports: Object.freeze([...(options.ports ?? [])]),
      });
    }
  }

  // Bun owns ports and structured clone. Worker facades add only document URL
  // resolution, frame delivery and termination when that document is replaced.
  const liveWorkers = new Set();
  const workerStates = new WeakMap();
  const workerHandlers = new WeakMap();
  const setWorkerHandler = (worker, type, callback) => {
    let handlers = workerHandlers.get(worker);
    if (!handlers) workerHandlers.set(worker, handlers = {});
    setEventHandler(worker, handlers, type, callback);
  };
  class Worker extends EventTarget {
    constructor(url, options = {}) {
      super();
      const entry = runtimeUrl(url);
      const worker = new host.Worker(entry, options);
      workerStates.set(this, { worker });
      liveWorkers.add(this);
      for (const type of ["message", "messageerror", "error"]) {
        worker.addEventListener(type, event => queueHostCompletion(() => {
          if (!liveWorkers.has(this)) return;
          this.dispatchEvent(type === "error" ? new ErrorEvent(type, event)
            : new MessageEvent(type, { data: event.data, ports: event.ports, origin: entry }));
        }));
      }
    }
    postMessage(message, options) { workerStates.get(this).worker.postMessage(message, options); }
    terminate() {
      if (!liveWorkers.delete(this)) return;
      const { worker } = workerStates.get(this);
      worker.terminate();
    }
    get onmessage() { return workerHandlers.get(this)?.message ?? null; }
    set onmessage(callback) { setWorkerHandler(this, "message", callback); }
    get onmessageerror() { return workerHandlers.get(this)?.messageerror ?? null; }
    set onmessageerror(callback) { setWorkerHandler(this, "messageerror", callback); }
    get onerror() { return workerHandlers.get(this)?.error ?? null; }
    set onerror(callback) { setWorkerHandler(this, "error", callback); }
  }
  const settlePorts = () => {};
  const portsPending = () => liveWorkers.size > 0 || hostCompletions.length > 0;

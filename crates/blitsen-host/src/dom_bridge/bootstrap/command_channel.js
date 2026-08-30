  // One command/completion protocol for every frame-settled native API. The
  // host-specific adapters below only decode their wire shape, build their
  // error type and deliver non-completion events; ID matching, stale answers,
  // deletion-before-callback, resolution and rejection happen here once.
  const deliverCommandListeners = (listeners, event, what) => {
    for (const listener of listeners) {
      try { listener(event); }
      catch (error) { console.error(`Uncaught exception in ${what} listener`, error); }
    }
  };
  // The registering half of the same contract, written once so every `onX` on
  // the native surface agrees: a listener is a function or the call is a
  // mistake, and what comes back is the function that removes it again.
  const commandListener = (listeners, listener, what) => {
    if (typeof listener !== "function") throw new TypeError(`${what} listener must be a function`);
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  };
  const makeCommandChannel = ({
    pending = () => false,
    take,
    decode = message => message,
    completion = message => message.type === "completion",
    commandId = message => message.commandId,
    result = message => message.value,
    rejected = message => message.error !== null,
    // One rejection type for the whole native surface, so an application
    // discriminates on `error.name` rather than on message text. A host that
    // names the failure keeps its name; one that does not says only that the
    // operation failed, which is what `OperationError` means.
    error = message => new DOMException(message.error, message.errorName ?? "OperationError"),
    onMessage = () => {},
    keepAlive = () => false,
    pollPendingCommands = false,
  }) => {
    const commands = new Map();
    const run = (id, { onComplete = null, transform = null } = {}) =>
      new Promise((resolve, reject) => {
        commands.set(String(id), { resolve, reject, onComplete, transform });
      });
    const settle = () => {
      if (!pending() && !(pollPendingCommands && commands.size > 0)) return;
      for (const raw of take()) {
        const message = decode(raw);
        if (!completion(message, raw)) {
          onMessage(message, raw);
          continue;
        }
        const key = String(commandId(message, raw));
        const command = commands.get(key);
        // A completion from a reset or otherwise superseded realm is stale. It
        // cannot settle a newer promise merely because its message was valid.
        if (!command) continue;
        commands.delete(key);
        const value = result(message, raw);
        command.onComplete?.(value, message.error);
        if (rejected(message, raw)) command.reject(error(message, raw));
        else command.resolve(command.transform ? command.transform(value) : value);
      }
    };
    const workPending = () => commands.size > 0 || keepAlive() || pending();
    const clear = () => { commands.clear(); };
    return Object.freeze({ run, settle, workPending, clear });
  };

  // The native harness exercises the protocol with fabricated host queues. No
  // application document receives this hook.
  if (testHarness) Object.defineProperty(globalThis, "__blitsenTestCommandChannel", {
    value: makeCommandChannel, configurable: true,
  });

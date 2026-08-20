  // EventSource. A server-sent event stream, read on the same worker pool
  // `fetch` and `WebSocket` run on and delivered at the same point in the frame
  // turn: the response, every event and every reconnection land at the start of
  // the animation-frame stage, so an application is never re-entered from a
  // network thread part-way through a frame.
  //
  // The transport keeps the parser, the reconnection timer and the last event
  // id (see `event_source.rs`). What arrives here is only what the application
  // can observe, which is why this half is a class over an event queue rather
  // than a protocol implementation.
  const eventSourceStates = new WeakMap();
  const eventSourceState = source => {
    const state = eventSourceStates.get(source);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };
  // A stream that is connecting or open keeps the host turning: its events land
  // in `animationFrameTick`, so a loop that idled would never deliver them. A
  // stream is dropped from this map when it is closed or has failed for good —
  // the two states from which nothing more is owed.
  const liveEventSources = new Map();
  const eventSourceHandlers = new WeakMap();
  const setEventSourceHandler = (source, type, callback) => {
    let handlers = eventSourceHandlers.get(source);
    if (!handlers) eventSourceHandlers.set(source, handlers = {});
    setEventHandler(source, handlers, type, callback);
  };
  // The one handoff point for stream work, for the same reason `fetch` has one.
  const settleEventSources = () => {
    if (liveEventSources.size === 0) return;
    for (const record of JSON.parse(__blitsenEventSourcePoll())) {
      const source = liveEventSources.get(record.id);
      if (!source) continue;
      const state = eventSourceState(source);
      if (record.type === "message") {
        // Named events are delivered as MessageEvent under their own type, and
        // the default type is what an `onmessage` handler is listening for.
        source.dispatchEvent(new MessageEvent(record.event, {
          data: record.data, lastEventId: record.lastEventId, origin: state.origin }));
        continue;
      }
      if (record.type === "open") {
        state.readyState = 1;
        source.dispatchEvent(new Event("open"));
        continue;
      }
      // An error is either the connection coming back — the stream is
      // CONNECTING again and the transport is already waiting out the retry
      // interval — or the end of it. The event itself carries no detail in a
      // browser and carries none here, but a stream that failed for a nameable
      // reason says so where a developer will see it rather than going quiet.
      if (record.fatal) {
        state.readyState = 2;
        liveEventSources.delete(record.id);
      } else {
        state.readyState = 0;
      }
      if (record.message)
        console.error(`EventSource connection to ${state.url} failed: ${record.message}`);
      source.dispatchEvent(new Event("error"));
    }
  };

  class EventSource extends EventTarget {
    constructor(url, options = {}) {
      super();
      const target = resolveAgainstDocument(url);
      if (target.protocol !== "http:" && target.protocol !== "https:")
        throw new DOMException(`${target.href} is not an http: or https: address`, "SyntaxError");
      // Reflected because the property is observable and code branches on it.
      // It withholds nothing: there is no cookie store and no per-origin
      // credential behind this runtime, so there is nothing for `false` to keep
      // back — see COMPATIBILITY.md.
      const withCredentials = Boolean(options?.withCredentials);
      const id = __blitsenEventSourceOpen(target.href);
      eventSourceStates.set(this, { id, readyState: 0, url: target.href,
        origin: target.origin, withCredentials });
      liveEventSources.set(id, this);
    }
    get url() { return eventSourceState(this).url; }
    get readyState() { return eventSourceState(this).readyState; }
    get withCredentials() { return eventSourceState(this).withCredentials; }
    close() {
      const state = eventSourceState(this);
      // Closing is idempotent and fires nothing: the state is the only
      // observable, and a stream already closed has nothing left to stop.
      if (state.readyState === 2) return;
      state.readyState = 2;
      liveEventSources.delete(state.id);
      __blitsenEventSourceClose(String(state.id));
    }
    get onopen() { return eventSourceHandlers.get(this)?.open ?? null; }
    set onopen(callback) { setEventSourceHandler(this, "open", callback); }
    get onmessage() { return eventSourceHandlers.get(this)?.message ?? null; }
    set onmessage(callback) { setEventSourceHandler(this, "message", callback); }
    get onerror() { return eventSourceHandlers.get(this)?.error ?? null; }
    set onerror(callback) { setEventSourceHandler(this, "error", callback); }
  }
  defineConstants(EventSource, ["CONNECTING", "OPEN", "CLOSED"]);

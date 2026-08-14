  // What a worker's global scope has before the shared fragments load.
  //
  // Events are flat here. The document's dispatcher walks a tree — capture,
  // target, bubble — because a DOM event has ancestors to travel through; a
  // worker's targets are the global scope and its ports, neither of which has a
  // parent. So this is the whole of dispatch rather than a cut-down copy of the
  // document's, and it is smaller because the job is smaller, not because
  // something was left out.
  const listenerMaps = new WeakMap();
  const listenersFor = target => {
    let listeners = listenerMaps.get(target);
    if (!listeners) listenerMaps.set(target, listeners = new Map());
    return listeners;
  };
  const listenerOptions = options =>
    typeof options === "object" && options !== null
      ? { once: Boolean(options.once) }
      : { once: false };

  const eventStates = new WeakMap();
  const stateFor = event => {
    const state = eventStates.get(event);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };

  class Event {
    constructor(type, options = {}) {
      eventStates.set(this, {
        type: String(type), target: null, currentTarget: null, eventPhase: 0,
        bubbles: Boolean(options.bubbles), cancelable: Boolean(options.cancelable),
        defaultPrevented: false, stopped: false, timeStamp: performance.now(),
      });
    }
    get type() { return stateFor(this).type; }
    get target() { return stateFor(this).target; }
    get currentTarget() { return stateFor(this).currentTarget; }
    get eventPhase() { return stateFor(this).eventPhase; }
    get bubbles() { return stateFor(this).bubbles; }
    get cancelable() { return stateFor(this).cancelable; }
    get defaultPrevented() { return stateFor(this).defaultPrevented; }
    get timeStamp() { return stateFor(this).timeStamp; }
    preventDefault() {
      const state = stateFor(this);
      if (state.cancelable) state.defaultPrevented = true;
    }
    stopPropagation() { stateFor(this).stopped = true; }
    stopImmediatePropagation() { stateFor(this).stopped = true; }
  }

  class EventTarget {
    addEventListener(type, callback, options = false) {
      if (typeof callback !== "function" && typeof callback?.handleEvent !== "function") return;
      const listeners = listenersFor(this);
      const key = String(type);
      const existing = listeners.get(key) ?? [];
      if (existing.some(record => record.callback === callback)) return;
      existing.push({ callback, ...listenerOptions(options) });
      listeners.set(key, existing);
    }
    removeEventListener(type, callback) {
      const listeners = listenerMaps.get(this)?.get(String(type));
      if (!listeners) return;
      const index = listeners.findIndex(record => record.callback === callback);
      if (index >= 0) listeners.splice(index, 1);
    }
    dispatchEvent(event) {
      const state = stateFor(event);
      state.target = this;
      state.currentTarget = this;
      state.eventPhase = 2;
      // Copied before the walk: a listener that removes another must not change
      // the set this dispatch was started with.
      for (const record of [...(listenerMaps.get(this)?.get(state.type) ?? [])]) {
        if (state.stopped) break;
        if (record.once) this.removeEventListener(state.type, record.callback);
        try {
          if (typeof record.callback === "function") record.callback.call(this, event);
          else record.callback.handleEvent(event);
        } catch (error) {
          // One broken listener must not stop the others, and must not throw
          // back into the host turn that delivered the event.
          globalThis.reportError(error);
        }
      }
      state.currentTarget = null;
      state.eventPhase = 0;
      return !state.defaultPrevented;
    }
  }

  // This worker's own address, which everything relative resolves against — its
  // script's URL, exactly as it is in a browser.
  const workerIdentity = JSON.parse(__blitsenWorkerIdentity);
  const workerUrl = workerIdentity.url;
  const resolveAgainstDocument = url => JSON.parse(__blitsenResolveUrl(workerUrl, String(url)));
  const resolveWorkerUrl = url => resolveAgainstDocument(url).href;
  const messageOrigin = resolveAgainstDocument("/").origin;

  // `window.stop()` is the one thing in the shared `fetch` fragment that speaks
  // to a document. There is none on this thread, and saying so is better than a
  // ReferenceError naming an internal helper.
  const call = operation => {
    throw new TypeError(`there is no document on a worker's thread to ${operation}`);
  };

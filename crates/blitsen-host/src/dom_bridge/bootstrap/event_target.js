  class EventTarget {
    addEventListener(type, callback, options = false) {
      if (!validListener(callback)) return;
      type = String(type);
      const normalized = listenerOptions(options);
      const map = listenersFor(this);
      const listeners = map.get(type) ?? [];
      if (listeners.some(record => !record.removed && record.callback === callback && record.capture === normalized.capture))
        return;
      listeners.push({ callback, ...normalized, removed: false });
      map.set(type, listeners);
    }
    removeEventListener(type, callback, options = false) {
      if (!validListener(callback)) return;
      type = String(type);
      const capture = listenerOptions(options).capture;
      const record = listenerMaps.get(this)?.get(type)?.find(record =>
        !record.removed && record.callback === callback && record.capture === capture);
      if (record) removeListenerRecord(this, type, record);
    }
    dispatchEvent(event) { return dispatchTo(this, event); }
  }

  const mutationObservers = new Set();
  const isObservedTarget = (observed, target, subtree) => {
    if (observed === target) return true;
    if (!subtree) return false;
    for (let ancestor = target?.parentNode; ancestor; ancestor = ancestor.parentNode)
      if (ancestor === observed) return true;
    return false;
  };
  const notifyMutation = record => {
    windowModesTreeMutation();
    for (const observer of mutationObservers) {
      if (!observer._observations.some(({ target, options }) =>
        options[record.type] && isObservedTarget(target, record.target, options.subtree))) continue;
      observer._records.push(Object.freeze(record));
      if (observer._queued) continue;
      observer._queued = true;
      queueMicrotask(() => {
        observer._queued = false;
        const records = observer.takeRecords();
        if (records.length > 0 && observer._observations.length > 0)
          observer._callback(records, observer);
      });
    }
  };

  class MutationObserver {
    constructor(callback) {
      if (typeof callback !== "function") throw new TypeError("MutationObserver callback must be a function");
      this._callback = callback;
      this._observations = [];
      this._records = [];
      this._queued = false;
    }
    observe(target, options = {}) {
      if (!(target instanceof Node) && target !== document)
        throw new TypeError("MutationObserver target must be a Node");
      const normalized = {
        childList: Boolean(options.childList), attributes: Boolean(options.attributes),
        characterData: Boolean(options.characterData), subtree: Boolean(options.subtree),
      };
      if (!normalized.childList && !normalized.attributes && !normalized.characterData)
        throw new TypeError("MutationObserver options must enable at least one mutation type");
      this._observations = this._observations.filter(observation => observation.target !== target);
      this._observations.push({ target, options: normalized });
      mutationObservers.add(this);
    }
    disconnect() {
      this._observations = [];
      this._records = [];
      mutationObservers.delete(this);
    }
    takeRecords() { return this._records.splice(0); }
  }

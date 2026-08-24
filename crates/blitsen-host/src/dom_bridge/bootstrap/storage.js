  // Storage. sessionStorage belongs to this realm; localStorage delegates each
  // synchronous operation to the application's atomic keyed-file store when a
  // running application supplied one. A bare bridge harness has no application
  // identity and keeps the previous in-memory behavior.
  const storageEntries = new WeakMap();
  const persistentStorage = new WeakSet();
  const entriesOf = storage => {
    const entries = storageEntries.get(storage);
    if (!entries) throw new TypeError("Illegal invocation");
    return entries;
  };

  class Storage {
    constructor() { storageEntries.set(this, new Map()); }
    get length() { return storageKeys(this).length; }
    key(index) { return storageKeys(this)[Number(index)] ?? null; }
    getItem(key) {
      key = String(key);
      return persistentStorage.has(this) ? __blitsenStorageGet(key) : entriesOf(this).get(key) ?? null;
    }
    setItem(key, value) {
      key = String(key); value = String(value);
      if (persistentStorage.has(this)) __blitsenStorageSet(key, value);
      else entriesOf(this).set(key, value);
    }
    removeItem(key) {
      key = String(key);
      if (persistentStorage.has(this)) __blitsenStorageRemove(key);
      else entriesOf(this).delete(key);
    }
    clear() {
      if (persistentStorage.has(this)) __blitsenStorageClear();
      else entriesOf(this).clear();
    }
  }

  const storageKeys = storage => persistentStorage.has(storage)
    ? JSON.parse(__blitsenStorageKeys()) : [...entriesOf(storage).keys()];

  // Property access is the same store as `getItem`, so `storage.theme = "dark"`
  // cannot diverge from `storage.setItem("theme", "dark")`.
  const storageArea = persistent => {
    const storage = new Storage();
    if (persistent) persistentStorage.add(storage);
    const area = new Proxy(storage, {
      get(target, key, receiver) {
        return typeof key !== "string" || key in target
          ? Reflect.get(target, key, receiver) : target.getItem(key) ?? undefined;
      },
      set(target, key, value, receiver) {
        if (typeof key !== "string" || key in target) return Reflect.set(target, key, value, receiver);
        target.setItem(key, value);
        return true;
      },
      has(target, key) {
        return key in target || (typeof key === "string" && target.getItem(key) !== null);
      },
      deleteProperty(target, key) { target.removeItem(key); return true; },
      ownKeys(target) { return storageKeys(target); },
      getOwnPropertyDescriptor(target, key) {
        const value = typeof key === "string" ? target.getItem(key) : null;
        return value === null ? undefined : { value, writable: true, enumerable: true, configurable: true };
      },
    });
    // A method reached through the proxy is called with the proxy as `this`, so
    // both objects have to find the same entries.
    storageEntries.set(area, entriesOf(storage));
    if (persistent) persistentStorage.add(area);
    return area;
  };
  const localStorage = storageArea(typeof __blitsenStorageGet === "function");
  const sessionStorage = storageArea(false);

  // Identity, never capability. These three are facts about the machine the
  // application is running on, which is why they can be answered at all; every
  // capability `navigator` normally carries stays absent so that feature
  // detection still selects a fallback path.
  const navigatorFacts = JSON.parse(__blitsenNavigatorState);

  class Navigator {
    constructor() { throw new TypeError("Illegal constructor"); }
    get userAgent() { return navigatorFacts.userAgent; }
    get platform() { return navigatorFacts.platform; }
    get language() { return navigatorFacts.language; }
    get languages() { return Object.freeze([navigatorFacts.language]); }
    getGamepads() { return gamepadSnapshots(); }
  }

  const navigator = Object.create(Navigator.prototype);

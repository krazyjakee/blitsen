  // Storage. In memory for the life of the process, and no more than that:
  // there is no profile directory behind an exported application yet, so
  // `localStorage` here keeps a session rather than a preference. The reason it
  // exists at all is that its absence is not survivable — libraries read it
  // unguarded inside a render — while its forgetfulness is, and `doctor` reports
  // that forgetfulness on every build rather than leaving it to be discovered.
  const storageEntries = new WeakMap();
  const entriesOf = storage => {
    const entries = storageEntries.get(storage);
    if (!entries) throw new TypeError("Illegal invocation");
    return entries;
  };

  class Storage {
    constructor() { storageEntries.set(this, new Map()); }
    get length() { return entriesOf(this).size; }
    key(index) { return [...entriesOf(this).keys()][Number(index)] ?? null; }
    getItem(key) { return entriesOf(this).get(String(key)) ?? null; }
    setItem(key, value) { entriesOf(this).set(String(key), String(value)); }
    removeItem(key) { entriesOf(this).delete(String(key)); }
    clear() { entriesOf(this).clear(); }
  }

  // Property access is the same store as `getItem`, so `storage.theme = "dark"`
  // cannot diverge from `storage.setItem("theme", "dark")`.
  const storageArea = () => {
    const storage = new Storage();
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
      ownKeys(target) { return [...entriesOf(target).keys()]; },
      getOwnPropertyDescriptor(target, key) {
        const value = typeof key === "string" ? target.getItem(key) : null;
        return value === null ? undefined : { value, writable: true, enumerable: true, configurable: true };
      },
    });
    // A method reached through the proxy is called with the proxy as `this`, so
    // both objects have to find the same entries.
    storageEntries.set(area, entriesOf(storage));
    return area;
  };
  const localStorage = storageArea();
  const sessionStorage = storageArea();

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
  }

  const navigator = Object.create(Navigator.prototype);


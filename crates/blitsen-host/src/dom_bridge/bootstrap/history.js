  // Location and history. In-memory only: no navigation, no network, no
  // back-forward cache. The address is synthetic because an exported
  // application has no server and therefore no origin; it is path-rooted
  // because that is what a client-side router reads.
  const documentUrl = call("documentUrl");
  let historyEntries = [{ url: documentUrl, state: null }];
  let historyIndex = 0;
  let scrollRestoration = "auto";
  let locationParts = call("urlParts", documentUrl);
  const currentUrl = () => historyEntries[historyIndex].url;
  const refreshLocation = () => { locationParts = call("urlParts", currentUrl()); };
  const resolveAgainstDocument = url => call("resolveUrl", currentUrl(), String(url));
  const sameDocumentTarget = url => {
    const target = resolveAgainstDocument(url);
    if (!target.sameOrigin)
      throw new DOMException(`cannot reach ${target.href} from ${currentUrl()}`, "SecurityError");
    return target.href;
  };
  const pushEntry = (url, state) => {
    historyEntries.length = historyIndex + 1;
    historyEntries.push({ url, state });
    historyIndex = historyEntries.length - 1;
    refreshLocation();
  };

  class PopStateEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, { state: options.state ?? null });
    }
  }

  class HashChangeEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, {
        oldURL: String(options.oldURL ?? ""),
        newURL: String(options.newURL ?? ""),
      });
    }
  }

  const traverseHistory = delta => {
    const next = Math.min(historyEntries.length - 1, Math.max(0, historyIndex + delta));
    if (next === historyIndex) return;
    const previous = locationParts;
    historyIndex = next;
    refreshLocation();
    globalThis.dispatchEvent(new PopStateEvent("popstate", { state: historyEntries[historyIndex].state }));
    if (previous.hash !== locationParts.hash)
      globalThis.dispatchEvent(new HashChangeEvent("hashchange",
        { oldURL: previous.href, newURL: locationParts.href }));
  };

  class History {
    constructor() { throw new TypeError("Illegal constructor"); }
    get length() { return historyEntries.length; }
    get state() { return historyEntries[historyIndex].state; }
    get scrollRestoration() { return scrollRestoration; }
    set scrollRestoration(value) { if (value === "auto" || value === "manual") scrollRestoration = value; }
    pushState(state, unused, url) {
      pushEntry(url == null ? currentUrl() : sameDocumentTarget(url), state ?? null);
    }
    replaceState(state, unused, url) {
      historyEntries[historyIndex] = { url: url == null ? currentUrl() : sameDocumentTarget(url), state: state ?? null };
      refreshLocation();
    }
    // Traversal is a task on the web, and routers rely on observing popstate
    // after their own call returns.
    go(delta = 0) { setTimeout(() => traverseHistory(Math.trunc(Number(delta)) || 0), 0); }
    back() { this.go(-1); }
    forward() { this.go(1); }
  }

  const noDocumentNavigation = property => {
    throw new DOMException(
      `Blitsen has no document navigation; use history.pushState instead of assigning location.${property}`,
      "NotSupportedError");
  };

  class Location {
    constructor() { throw new TypeError("Illegal constructor"); }
    get href() { return locationParts.href; }
    set href(value) { noDocumentNavigation("href"); }
    get protocol() { return locationParts.protocol; }
    get host() { return locationParts.host; }
    get hostname() { return locationParts.hostname; }
    get port() { return locationParts.port; }
    get origin() { return locationParts.origin; }
    get pathname() { return locationParts.pathname; }
    set pathname(value) { noDocumentNavigation("pathname"); }
    get search() { return locationParts.search; }
    set search(value) { noDocumentNavigation("search"); }
    get hash() { return locationParts.hash; }
    set hash(value) {
      const text = String(value);
      const target = sameDocumentTarget(text.startsWith("#") ? text : `#${text}`);
      if (target === currentUrl()) return;
      const previous = locationParts;
      pushEntry(target, null);
      globalThis.dispatchEvent(new HashChangeEvent("hashchange",
        { oldURL: previous.href, newURL: locationParts.href }));
    }
    toString() { return locationParts.href; }
  }

  const location = Object.create(Location.prototype);
  const history = Object.create(History.prototype);


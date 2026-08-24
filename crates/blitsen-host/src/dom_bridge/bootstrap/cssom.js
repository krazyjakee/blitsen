  class CSSStyleDeclaration {
    constructor(element) { this._element = element; }
    _name(property) { const name = String(property); return name.startsWith("--") ? name : name.toLowerCase(); }
    getPropertyValue(property) { return call("styleGet", this._element[handle], this._name(property)); }
    setProperty(property, value) { call("styleSet", this._element[handle], this._name(property), String(value)); }
    removeProperty(property) { return call("styleRemove", this._element[handle], this._name(property)); }
    get cssText() { return call("styleText", this._element[handle]); }
    set cssText(value) { call("setStyleText", this._element[handle], String(value)); }
    _getJsProperty(property) { return call("styleGetJs", this._element[handle], property); }
    _setJsProperty(property, value) { call("styleSetJs", this._element[handle], property, value); }
  }

  // The CSSOM stylesheet objects, at the size the frameworks that need them use:
  // Svelte writes a `@keyframes` block into a sheet it owns for every transition
  // it runs, and takes the sheet off `styleElement.sheet`.
  //
  // A sheet here is its owning element. The rules are the element's text, which
  // is what Blitz parses and hands to Stylo, so an inserted rule is in the same
  // stylesheet set the cascade reads and there is no shadow copy to fall out of
  // step. Because of that, `cssRules` is derived from the sheet's current source
  // on every read: the list object handed out is a frozen snapshot like every
  // other collection here, but the next read sees the mutation, so an index
  // computed from `cssRules.length` and passed straight to `insertRule` means
  // what it does in a browser.
  //
  // What stays absent is the rest of CSSOM: rule subclasses, `rule.style`,
  // `selectorText`, `disabled`, constructible sheets, and the rules of a sheet
  // loaded from a URL, whose source is a file rather than text in the tree.
  const sheetOwners = new WeakMap();
  const sheetToken = Symbol("Blitsen stylesheet");
  const ownerOf = sheet => {
    const owner = sheetOwners.get(sheet);
    if (!owner) throw new TypeError("Illegal invocation");
    return owner;
  };
  const ruleText = new WeakMap();

  class CSSRule {
    constructor(token, cssText, parentStyleSheet) {
      if (token !== sheetToken) throw new TypeError("Illegal constructor");
      ruleText.set(this, { cssText: String(cssText), parentStyleSheet });
      Object.freeze(this);
    }
    get cssText() { return ruleText.get(this).cssText; }
    get parentStyleSheet() { return ruleText.get(this).parentStyleSheet; }
    toString() { return this.cssText; }
  }

  class CSSRuleList {
    constructor(rules) {
      defineIndexed(this, rules);
    }
    item(index) { return this[index] ?? null; }
    *[Symbol.iterator]() { for (let index = 0; index < this.length; index++) yield this[index]; }
  }

  class CSSStyleSheet {
    // Constructible stylesheets need `adoptedStyleSheets` to reach the cascade,
    // and that is absent, so a sheet only ever comes from an element. Throwing
    // is what lets `new CSSStyleSheet()` feature-detection pick its fallback.
    constructor(token, owner) {
      if (token !== sheetToken)
        throw new TypeError("constructible stylesheets are not implemented; "
          + "append a <style> element and use its sheet");
      sheetOwners.set(this, owner);
    }
    get ownerNode() { return ownerOf(this); }
    get href() {
      const owner = ownerOf(this);
      return elementTag(owner) === "link" ? owner.href : null;
    }
    get parentStyleSheet() { return null; }
    get type() { return "text/css"; }
    get title() { return ownerOf(this).getAttribute("title"); }
    get cssRules() {
      return new CSSRuleList(call("sheetRules", ownerOf(this)[handle])
        .map(text => new CSSRule(sheetToken, text, this)));
    }
    get rules() { return this.cssRules; }
    insertRule(rule, index = 0) {
      const owner = ownerOf(this);
      const position = Number(index);
      if (!Number.isInteger(position) || position < 0 || position > this.cssRules.length)
        throw new DOMException(`cannot insert a rule at ${index}`, "IndexSizeError");
      try { call("insertSheetRule", owner[handle], String(rule), position); }
      catch (error) { throw new DOMException(String(error.message ?? error), "SyntaxError"); }
      return position;
    }
    deleteRule(index) {
      const owner = ownerOf(this);
      const position = Number(index);
      if (!Number.isInteger(position) || position < 0 || position >= this.cssRules.length)
        throw new DOMException(`no rule at ${index}`, "IndexSizeError");
      call("deleteSheetRule", owner[handle], position);
    }
  }

  class StyleSheetList {
    constructor(sheets) {
      defineIndexed(this, sheets);
    }
    item(index) { return this[index] ?? null; }
    *[Symbol.iterator]() { for (let index = 0; index < this.length; index++) yield this[index]; }
  }

  // One sheet object per element, for the whole of the element's life: Svelte
  // keeps the sheet it made and later detaches `sheet.ownerNode`, which only
  // works if the sheet it kept still knows which element it came from.
  const sheetCache = new WeakMap();
  const sheetFor = element => {
    let sheet = sheetCache.get(element);
    if (!sheet) {
      sheet = new CSSStyleSheet(sheetToken, element);
      sheetCache.set(element, sheet);
    }
    return sheet;
  };

  // Computed style. Blitz has already resolved the cascade, so this reads that
  // answer back rather than keeping a second idea of what an element's style is.
  // Every read is layout-dependent — `width` and `height` resolve to the used
  // value — so it takes the same flush the geometry reads take, and a read after
  // a write counts as the forced layout it is.
  const readOnlyStyle = () => {
    throw new DOMException("a computed style declaration is read-only", "NoModificationAllowedError");
  };

  class CSSResolvedStyleDeclaration extends CSSStyleDeclaration {
    // An empty string here is what a browser returns too: an unknown property,
    // an unset custom property, or a shorthand whose longhands do not compose.
    // The one case a browser answers differently is an element the cascade has
    // never reached — see COMPATIBILITY.md.
    getPropertyValue(property) {
      return recordForcedLayout(
        call("computedStyle", this._element[handle], this._name(property))).value ?? "";
    }
    // CSSOM: a computed declaration block serializes as nothing.
    get cssText() { return ""; }
    set cssText(value) { readOnlyStyle(); }
    setProperty(property, value) { readOnlyStyle(); }
    removeProperty(property) { readOnlyStyle(); }
    _getJsProperty(property) {
      return recordForcedLayout(
        call("computedStyleJs", this._element[handle], property)).value ?? "";
    }
    _setJsProperty(property, value) { readOnlyStyle(); }
  }

  const computedStyleCache = new WeakMap();
  const getComputedStyle = (element, pseudoElement = null) => {
    if (!(element instanceof Element)) throw new TypeError("getComputedStyle requires an Element");
    // A pseudo-element box is not addressable through this bridge, and answering
    // with the originating element's style would be a wrong answer rather than
    // a missing one.
    if (pseudoElement != null && String(pseudoElement) !== "")
      throw new DOMException(`no resolved style for ${pseudoElement}`, "NotSupportedError");
    let style = computedStyleCache.get(element);
    if (!style) {
      style = new Proxy(new CSSResolvedStyleDeclaration(element), {
        get(target, property, receiver) {
          if (typeof property !== "string" || property in target) return Reflect.get(target, property, receiver);
          return target._getJsProperty(property);
        },
        set() { readOnlyStyle(); },
      });
      computedStyleCache.set(element, style);
    }
    return style;
  };

  // Media queries. Stylo evaluates `@media` for the cascade; this asks it the
  // same question from JavaScript, so a feature the style engine does not
  // implement is unknown to both and its query does not match.
  const mediaQueryLists = new Set();
  const mediaStates = new WeakMap();
  const mediaStateFor = list => {
    const state = mediaStates.get(list);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };

  class MediaQueryListEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, {
        media: String(options.media ?? ""),
        matches: Boolean(options.matches),
      });
    }
  }

  class MediaQueryList extends EventTarget {
    constructor() { throw new TypeError("Illegal constructor"); }
    get media() { return mediaStateFor(this).media; }
    get matches() { return mediaStateFor(this).matches; }
    get onchange() { return mediaStateFor(this).onchange; }
    set onchange(callback) {
      const state = mediaStateFor(this);
      setEventHandler(this, state, "change", callback, "onchange");
    }
    // A list is only worth re-evaluating once something is listening to it.
    addEventListener(type, callback, options = false) {
      super.addEventListener(type, callback, options);
      mediaQueryLists.add(this);
    }
    // The pre-2019 spelling, which a library still installs when its own type
    // definitions predate `addEventListener` on this interface.
    addListener(callback) { this.addEventListener("change", callback); }
    removeListener(callback) { this.removeEventListener("change", callback); }
  }

  const matchMedia = query => {
    query = String(query);
    const list = Object.create(MediaQueryList.prototype);
    mediaStates.set(list, { query, onchange: null, ...call("matchMedia", query) });
    return list;
  };
  // The only device state an exported application can change is the viewport:
  // the colour scheme is fixed for the life of the process, so a query can only
  // flip when the window does.
  let mediaViewport = null;
  const notifyMediaQueries = () => {
    const viewport = `${innerWidth}x${innerHeight}@${devicePixelRatio}`;
    if (viewport === mediaViewport) return;
    mediaViewport = viewport;
    for (const list of mediaQueryLists) {
      const state = mediaStateFor(list);
      const { matches } = call("matchMedia", state.query);
      if (matches === state.matches) continue;
      state.matches = matches;
      list.dispatchEvent(new MediaQueryListEvent("change", { media: state.media, matches }));
    }
  };

  // Element resize observation, delivered at the top of the frame turn beside
  // the surface resizes, which is where this runtime settles geometry.
  const resizeObservers = new Set();
  const resizeSignature = (metrics, box) => box === "border-box"
    ? `${metrics.width}x${metrics.height}`
    : `${metrics.contentWidth}x${metrics.contentHeight}`;
  const resizeEntry = (target, metrics) => {
    const { contentX: x, contentY: y, contentWidth: width, contentHeight: height } = metrics;
    return Object.freeze({
      target,
      contentRect: Object.freeze({ x, y, width, height,
        top: y, right: x + width, bottom: y + height, left: x }),
      // Physical writing modes only: inline is width and block is height, which
      // holds for every writing mode this renderer lays out.
      borderBoxSize: Object.freeze([
        Object.freeze({ inlineSize: metrics.width, blockSize: metrics.height })]),
      contentBoxSize: Object.freeze([Object.freeze({ inlineSize: width, blockSize: height })]),
    });
  };
  // An element that has never been reported is work the frame loop owes the
  // application, the way an in-flight request is.
  const pendingResizeObservations = () => {
    let pending = 0;
    for (const observer of resizeObservers)
      for (const record of observer._targets.values()) if (record.reported === null) pending++;
    return pending;
  };
  const notifyResizeObservers = () => {
    const handles = new Set();
    for (const observer of resizeObservers)
      for (const target of observer._targets.keys()) handles.add(target[handle]);
    if (handles.size === 0) return;
    const metricsByHandle = new Map(call("resizeObserverMetrics",
      JSON.stringify([...handles])).map(metrics => [metrics.handle, metrics]));
    for (const observer of resizeObservers) {
      const entries = [];
      for (const [target, record] of observer._targets) {
        const metrics = metricsByHandle.get(target[handle]);
        if (metrics === undefined) continue;
        const signature = resizeSignature(metrics, record.box);
        if (signature === record.reported) continue;
        record.reported = signature;
        entries.push(resizeEntry(target, metrics));
      }
      if (entries.length === 0) continue;
      try { observer._callback(entries, observer); }
      catch (error) { console.error("Uncaught exception in ResizeObserver callback", error); }
    }
  };

  class ResizeObserver {
    constructor(callback) {
      if (typeof callback !== "function") throw new TypeError("ResizeObserver callback must be a function");
      this._callback = callback;
      this._targets = new Map();
    }
    observe(target, options = {}) {
      if (!(target instanceof Element)) throw new TypeError("ResizeObserver target must be an Element");
      const box = String(options.box ?? "content-box");
      // `device-pixel-content-box` needs a device-pixel snap this bridge does
      // not report, so it is refused rather than answered in CSS pixels.
      if (box !== "content-box" && box !== "border-box")
        throw new TypeError(`unsupported ResizeObserver box: ${box}`);
      this._targets.set(target, { box, reported: null });
      resizeObservers.add(this);
    }
    unobserve(target) { this._targets.delete(target); }
    disconnect() { this._targets.clear(); resizeObservers.delete(this); }
  }

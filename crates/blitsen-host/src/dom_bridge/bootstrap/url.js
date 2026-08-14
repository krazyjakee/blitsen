  // `URL` and `URLSearchParams`.
  //
  // Parsing is the Rust `url` crate, reached through the same `urlParts` and
  // `resolveUrl` operations `location` and `history` read (`web_url.rs`). So an
  // application that names an asset the idiomatic way —
  // `new URL("./blip.wav", import.meta.url)` — resolves it against the same
  // origin the module resolver and the renderer already use, and gets the same
  // answer on both hosts.
  //
  // It is here rather than inherited because `URL` is a Web IDL API and not an
  // ECMAScript one: the Phase 1 host happened to supply Bun's, and the engine
  // the shipped runtime hosts has none. An application that shipped a `.wav`
  // beside its module could not name it (#125), and the two hosts disagreed
  // about a global (#90).
  //
  // Object URLs stay absent. A `blob:` URL is a handle into a store that a
  // later `fetch` reads, and there is no origin behind an application to hang
  // one on — feature-detect `URL.createObjectURL` and pass the `Blob` itself,
  // or a data URL.
  const urlStates = new WeakMap();
  const urlStateOf = url => {
    const state = urlStates.get(url);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };

  const parseUrl = (href, base) => {
    try {
      return base === undefined
        ? call("urlParts", String(href))
        : call("resolveUrl", String(base), String(href));
    } catch {
      return null;
    }
  };

  // Puts a URL back together after a setter changed one component of it, so the
  // parser rather than this file decides what the result normalizes to.
  const composeUrl = parts => {
    if (parts.opaque) return `${parts.protocol}${parts.pathname}${parts.search}${parts.hash}`;
    const credentials = parts.username === "" && parts.password === ""
      ? ""
      : `${parts.username}${parts.password === "" ? "" : `:${parts.password}`}@`;
    return `${parts.protocol}//${credentials}${parts.host}`
      + `${parts.pathname}${parts.search}${parts.hash}`;
  };

  // A setter given something that does not parse leaves the URL alone, which is
  // what the specification says and what a router depends on: `url.port = "x"`
  // must not turn a working address into a broken one.
  const rewriteUrl = (state, changes) => {
    const parsed = parseUrl(composeUrl({ ...state.parts, ...changes }));
    if (parsed === null) return;
    state.parts = parsed;
    if (state.params !== null) syncParams(state.params, parsed.search);
  };

  const PARAM_ESCAPES = /[!'()~]/g;
  const encodeParam = value => encodeURIComponent(value)
    .replace(/%20/g, "+")
    .replace(PARAM_ESCAPES, character => `%${character.charCodeAt(0).toString(16).toUpperCase()}`);
  const decodeParam = value => {
    const plussed = String(value).replace(/\+/g, " ");
    // A percent sequence a bundle wrote by hand can be malformed; the query is
    // still readable around it, so the raw text is kept rather than throwing.
    try { return decodeURIComponent(plussed); } catch { return plussed; }
  };

  const paramStates = new WeakMap();
  const paramPairsOf = params => {
    const pairs = paramStates.get(params);
    if (!pairs) throw new TypeError("Illegal invocation");
    return pairs;
  };
  // Which URL a params object writes through, when it came from one. Mutating
  // `url.searchParams` updates `url.href`, and that is the whole reason the
  // object is worth having.
  const paramOwners = new WeakMap();

  const parsedParams = query => {
    const pairs = [];
    for (const field of String(query).replace(/^\?/, "").split("&")) {
      if (field === "") continue;
      const split = field.indexOf("=");
      if (split < 0) pairs.push([decodeParam(field), ""]);
      else pairs.push([decodeParam(field.slice(0, split)), decodeParam(field.slice(split + 1))]);
    }
    return pairs;
  };

  const serializedParams = pairs =>
    pairs.map(([name, value]) => `${encodeParam(name)}=${encodeParam(value)}`).join("&");

  // Reads a query back into a params object without writing it out again, which
  // is what a change to `url.search` has to do to its live `searchParams`.
  const syncParams = (params, query) => {
    const pairs = paramPairsOf(params);
    pairs.length = 0;
    for (const pair of parsedParams(query)) pairs.push(pair);
  };

  const paramsChanged = params => {
    const owner = paramOwners.get(params);
    if (owner === undefined) return;
    const query = serializedParams(paramPairsOf(params));
    const state = urlStates.get(owner);
    const parsed = parseUrl(composeUrl({ ...state.parts, search: query === "" ? "" : `?${query}` }));
    if (parsed !== null) state.parts = parsed;
  };

  class URLSearchParams {
    constructor(init) {
      paramStates.set(this, []);
      if (init === undefined || init === null) return;
      const pairs = paramPairsOf(this);
      if (typeof init === "string") {
        for (const pair of parsedParams(init)) pairs.push(pair);
        return;
      }
      if (init instanceof URLSearchParams) {
        for (const [name, value] of paramPairsOf(init)) pairs.push([name, value]);
        return;
      }
      if (typeof init[Symbol.iterator] === "function") {
        for (const pair of init) {
          const entry = [...pair];
          if (entry.length !== 2)
            throw new TypeError("URLSearchParams entries must be [name, value] pairs");
          pairs.push([String(entry[0]), String(entry[1])]);
        }
        return;
      }
      if (typeof init !== "object") throw new TypeError("invalid URLSearchParams initializer");
      for (const name of Object.keys(init)) pairs.push([name, String(init[name])]);
    }
    get size() { return paramPairsOf(this).length; }
    append(name, value) {
      paramPairsOf(this).push([String(name), String(value)]);
      paramsChanged(this);
    }
    delete(name, value) {
      const key = String(name);
      const pairs = paramPairsOf(this);
      const kept = pairs.filter(([field, held]) =>
        field !== key || (value !== undefined && held !== String(value)));
      pairs.length = 0;
      for (const pair of kept) pairs.push(pair);
      paramsChanged(this);
    }
    get(name) {
      const found = paramPairsOf(this).find(([field]) => field === String(name));
      return found === undefined ? null : found[1];
    }
    getAll(name) {
      return paramPairsOf(this).filter(([field]) => field === String(name)).map(([, value]) => value);
    }
    has(name, value) {
      return paramPairsOf(this).some(([field, held]) =>
        field === String(name) && (value === undefined || held === String(value)));
    }
    set(name, value) {
      const key = String(name);
      const pairs = paramPairsOf(this);
      const at = pairs.findIndex(([field]) => field === key);
      const kept = pairs.filter(([field], index) => field !== key || index === at);
      if (at < 0) kept.push([key, String(value)]);
      else kept[kept.findIndex(([field]) => field === key)] = [key, String(value)];
      pairs.length = 0;
      for (const pair of kept) pairs.push(pair);
      paramsChanged(this);
    }
    sort() {
      const pairs = paramPairsOf(this);
      // Stable, and by code unit rather than by locale: `sort()` on a query is
      // defined as a byte-ordering, and this runtime has no locale anyway.
      const sorted = pairs
        .map((pair, index) => ({ pair, index }))
        .sort((left, right) => (left.pair[0] < right.pair[0] ? -1
          : left.pair[0] > right.pair[0] ? 1 : left.index - right.index))
        .map(({ pair }) => pair);
      pairs.length = 0;
      for (const pair of sorted) pairs.push(pair);
      paramsChanged(this);
    }
    forEach(callback, thisArg) {
      for (const [name, value] of [...paramPairsOf(this)]) callback.call(thisArg, value, name, this);
    }
    *entries() { for (const [name, value] of [...paramPairsOf(this)]) yield [name, value]; }
    *keys() { for (const [name] of this.entries()) yield name; }
    *values() { for (const [, value] of this.entries()) yield value; }
    [Symbol.iterator]() { return this.entries(); }
    toString() { return serializedParams(paramPairsOf(this)); }
  }

  class URL {
    constructor(input, base) {
      const parsed = parseUrl(input, base);
      if (parsed === null) {
        throw new TypeError(base === undefined
          ? `Invalid URL: ${input}`
          : `Invalid URL: ${input} against base ${base}`);
      }
      urlStates.set(this, { parts: parsed, params: null });
    }
    static canParse(input, base) { return parseUrl(input, base) !== null; }
    // The newer spelling of the same question, and the one a bundle reaches for
    // when it wants the URL rather than a boolean.
    static parse(input, base) {
      return parseUrl(input, base) === null ? null : new URL(input, base);
    }
    get href() { return urlStateOf(this).parts.href; }
    set href(value) {
      const parsed = parseUrl(value);
      if (parsed === null) throw new TypeError(`Invalid URL: ${value}`);
      const state = urlStateOf(this);
      state.parts = parsed;
      if (state.params !== null) syncParams(state.params, parsed.search);
    }
    get origin() { return urlStateOf(this).parts.origin; }
    get protocol() { return urlStateOf(this).parts.protocol; }
    set protocol(value) {
      const text = String(value);
      rewriteUrl(urlStateOf(this), { protocol: text.endsWith(":") ? text : `${text}:` });
    }
    get username() { return urlStateOf(this).parts.username; }
    set username(value) { rewriteUrl(urlStateOf(this), { username: String(value) }); }
    get password() { return urlStateOf(this).parts.password; }
    set password(value) { rewriteUrl(urlStateOf(this), { password: String(value) }); }
    get host() { return urlStateOf(this).parts.host; }
    set host(value) { rewriteUrl(urlStateOf(this), { host: String(value) }); }
    get hostname() { return urlStateOf(this).parts.hostname; }
    set hostname(value) {
      const state = urlStateOf(this);
      const port = state.parts.port;
      rewriteUrl(state, { host: port === "" ? String(value) : `${value}:${port}` });
    }
    get port() { return urlStateOf(this).parts.port; }
    set port(value) {
      const state = urlStateOf(this);
      const text = String(value);
      rewriteUrl(state,
        { host: text === "" ? state.parts.hostname : `${state.parts.hostname}:${text}` });
    }
    get pathname() { return urlStateOf(this).parts.pathname; }
    set pathname(value) {
      const state = urlStateOf(this);
      if (state.parts.opaque) return;
      const text = String(value);
      rewriteUrl(state, { pathname: text.startsWith("/") ? text : `/${text}` });
    }
    get search() { return urlStateOf(this).parts.search; }
    set search(value) {
      const text = String(value);
      const query = text === "" || text === "?" ? "" : (text.startsWith("?") ? text : `?${text}`);
      rewriteUrl(urlStateOf(this), { search: query });
    }
    get searchParams() {
      const state = urlStateOf(this);
      if (state.params === null) {
        state.params = new URLSearchParams(state.parts.search);
        paramOwners.set(state.params, this);
      }
      return state.params;
    }
    get hash() { return urlStateOf(this).parts.hash; }
    set hash(value) {
      const text = String(value);
      const fragment = text === "" || text === "#" ? "" : (text.startsWith("#") ? text : `#${text}`);
      rewriteUrl(urlStateOf(this), { hash: fragment });
    }
    toString() { return this.href; }
    toJSON() { return this.href; }
  }

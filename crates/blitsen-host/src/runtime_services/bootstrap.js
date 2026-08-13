// Runtime services for a host that supplies only ECMAScript.
//
// Phase 1 gets all of this from Bun. Phase 2 drops Bun's runtime, so what the
// DOM bootstrap and the application below it actually rely on has to be here
// instead — and only that. This is not a Node or browser shim: nothing is added
// because some other runtime has it, and anything Blitsen does not implement
// stays absent so feature detection keeps working (TECH.md §16.4).
(() => {
  const install = (name, value) =>
    Object.defineProperty(globalThis, name, {
      value,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  // Everything below this is absent from a bare context, so defining it can
  // only add. `console` is the exception and is installed over the engine's own
  // (see below), which is why the two are separate operations.
  const define = (name, value) => {
    if (name in globalThis) return;
    install(name, value);
  };

  // Console. Replaced rather than deferred to: a bare JavaScriptCore context
  // already has a `console`, but it is the Web Inspector's, and with no
  // debugger attached every call is silently discarded — which reads exactly
  // like an application whose logging is broken. Formatting is deliberately
  // shallow: the host writes lines to the standard streams, and a full
  // inspector belongs to a devtools story that does not exist yet.
  const format = value => {
    if (typeof value === "string") return value;
    if (value instanceof Error) {
      // The name and message first, then the frames. Not `stack` alone: V8
      // begins it with "Name: message" and both engines Blitsen hosts do not,
      // so an error logged under QuickJS printed its frames and never said what
      // went wrong. Checked rather than assumed, so a host whose stack already
      // carries the header does not repeat it.
      const heading = `${value.name}: ${value.message}`;
      const stack = value.stack ?? "";
      if (!stack) return heading;
      return stack.startsWith(`${value.name}`) ? stack : `${heading}\n${stack}`;
    }
    if (typeof value === "bigint") return `${value}n`;
    if (typeof value === "symbol" || typeof value === "function") return String(value);
    if (value === null || value === undefined || typeof value !== "object") return String(value);
    try {
      return JSON.stringify(value, (_, entry) =>
        typeof entry === "bigint" ? `${entry}n` : entry) ?? String(value);
    } catch {
      return Object.prototype.toString.call(value);
    }
  };
  const write = level => (...values) =>
    __blitsenConsoleWrite(level, values.map(format).join(" "));
  const counts = new Map();
  const timers = new Map();
  install("console", {
    log: write("log"),
    info: write("log"),
    debug: write("log"),
    dir: write("log"),
    warn: write("warn"),
    error: write("error"),
    trace: (...values) => write("error")(...values, new Error("trace").stack ?? ""),
    assert: (condition, ...values) => { if (!condition) write("error")("Assertion failed:", ...values); },
    count: (label = "default") => {
      counts.set(label, (counts.get(label) ?? 0) + 1);
      write("log")(`${label}: ${counts.get(label)}`);
    },
    countReset: (label = "default") => counts.delete(label),
    group: write("log"),
    groupCollapsed: write("log"),
    groupEnd: () => {},
    table: write("log"),
    time: (label = "default") => timers.set(label, __blitsenNow()),
    timeEnd: (label = "default") => {
      const started = timers.get(label);
      if (started === undefined) return write("warn")(`Timer "${label}" does not exist`);
      timers.delete(label);
      write("log")(`${label}: ${(__blitsenNow() - started).toFixed(3)}ms`);
    },
    timeLog: (label = "default", ...values) => {
      const started = timers.get(label);
      if (started === undefined) return write("warn")(`Timer "${label}" does not exist`);
      write("log")(`${label}: ${(__blitsenNow() - started).toFixed(3)}ms`, ...values);
    },
  });

  // `performance.now` is a monotonic clock in milliseconds from process start,
  // which is what the frame pipeline already hands JavaScript. `timeOrigin` is
  // the Unix time of that zero so the two clocks can be related.
  define("performance", {
    now: () => __blitsenNow(),
    get timeOrigin() { return __blitsenTimeOrigin(); },
    mark: () => {},
    measure: () => {},
    clearMarks: () => {},
    clearMeasures: () => {},
  });

  define("reportError", error => {
    __blitsenConsoleWrite("error", `Uncaught ${format(error)}`);
  });

  // `queueMicrotask` over the engine's own job queue. An exception thrown by
  // the callback is reported rather than turned into a rejected promise, which
  // is the observable difference between this and a bare `.then`.
  define("queueMicrotask", callback => {
    if (typeof callback !== "function") {
      throw new TypeError("queueMicrotask callback must be a function");
    }
    Promise.resolve().then(() => {
      try {
        callback();
      } catch (error) {
        globalThis.reportError(error);
      }
    });
  });

  // The legacy error names DOM operations still throw by. The numeric codes are
  // the frozen historical table; names added after it correctly report 0.
  const LEGACY_CODES = {
    IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4,
    InvalidCharacterError: 5, NoModificationAllowedError: 7, NotFoundError: 8,
    NotSupportedError: 9, InUseAttributeError: 10, InvalidStateError: 11,
    SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14,
    InvalidAccessError: 15, TypeMismatchError: 17, SecurityError: 18,
    NetworkError: 19, AbortError: 20, URLMismatchError: 21,
    QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24,
    DataCloneError: 25,
  };
  class DOMException extends Error {
    constructor(message = "", name = "Error") {
      super(message);
      Object.defineProperty(this, "name", { value: name, configurable: true, writable: true });
    }
    get code() { return LEGACY_CODES[this.name] ?? 0; }
  }
  Object.defineProperty(DOMException.prototype, Symbol.toStringTag, {
    value: "DOMException",
    configurable: true,
  });
  for (const [name, code] of Object.entries(LEGACY_CODES)) {
    Object.defineProperty(DOMException, name.replace(/([a-z])([A-Z])/g, "$1_$2")
      .replace(/_ERROR$/i, "_ERR").toUpperCase(), { value: code, enumerable: true });
  }
  define("DOMException", DOMException);

  // Timers. The queue itself is the host's, because the frame loop has to know
  // when the next one is due; these are the shapes the web specifies around it.
  const timeout = (schedule, name) => (callback, delay = 0, ...args) => {
    if (typeof callback !== "function") {
      throw new TypeError(`${name} callback must be a function`);
    }
    const milliseconds = Number(delay);
    return schedule(callback, Number.isFinite(milliseconds) ? Math.max(0, milliseconds) : 0, ...args);
  };
  define("setTimeout", timeout(__blitsenSetTimeout, "setTimeout"));
  define("setInterval", timeout(__blitsenSetInterval, "setInterval"));
  define("clearTimeout", id => { if (id !== undefined && id !== null) __blitsenClearTimer(Number(id)); });
  define("clearInterval", id => { if (id !== undefined && id !== null) __blitsenClearTimer(Number(id)); });
})();

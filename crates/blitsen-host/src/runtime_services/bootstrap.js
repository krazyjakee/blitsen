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
  // only add. `console` is the exception and is installed outright (see below),
  // which is why the two are separate operations.
  const define = (name, value) => {
    if (name in globalThis) return;
    install(name, value);
  };

  // Console. Installed rather than deferred to, so it is this one or none: an
  // engine's own `console` is whatever its embedder wired up, and a host that
  // inherited one reads exactly like an application whose logging is broken.
  // QuickJS-ng's is in its `libc` helpers, which this host does not install, so
  // today the bare context has none at all. Formatting is deliberately shallow:
  // the host writes lines to the standard streams, and a full inspector belongs
  // to a devtools story that does not exist yet.
  const format = value => {
    if (typeof value === "string") return value;
    if (value instanceof Error) {
      // The name and message first, then the frames. Not `stack` alone: V8
      // begins it with "Name: message" and QuickJS does not, so an error logged
      // here printed its frames and never said what went wrong. Checked rather
      // than assumed, so a host whose stack already carries the header does not
      // repeat it.
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

  // `crypto`, at the two entry points the platform's randomness is reached
  // through. Not a WebCrypto implementation: `subtle` stays absent, so code
  // that needs signing or digests still selects its own fallback rather than
  // calling a stub. What is here is what a bare context cannot produce at all —
  // Monaco asks for both while it is starting, and without them an export was a
  // white window while the same application under Phase 1 borrowed Bun's.
  const RANDOM_VIEWS = new Set(["Int8Array", "Uint8Array", "Uint8ClampedArray",
    "Int16Array", "Uint16Array", "Int32Array", "Uint32Array",
    "BigInt64Array", "BigUint64Array"]);
  const HEX = Array.from({ length: 256 }, (_, byte) => byte.toString(16).padStart(2, "0"));
  define("crypto", {
    getRandomValues(view) {
      // The brand rather than `instanceof`: a view from another realm is still
      // a view, and a `Float64Array` is not one this accepts whichever realm
      // it came from.
      const kind = Object.prototype.toString.call(view).slice(8, -1);
      if (!RANDOM_VIEWS.has(kind)) {
        throw new DOMException(
          `crypto.getRandomValues does not accept ${kind}`, "TypeMismatchError");
      }
      if (view.byteLength > 65536) {
        throw new DOMException(
          `crypto.getRandomValues cannot fill ${view.byteLength} bytes at once`,
          "QuotaExceededError");
      }
      // Written through a byte view of the same memory, so the element type
      // above is irrelevant to the fill and the caller's array is the one that
      // changes — `getRandomValues` returns the view it was given.
      new Uint8Array(view.buffer, view.byteOffset, view.byteLength)
        .set(__blitsenRandomBytes(view.byteLength));
      return view;
    },
    randomUUID() {
      const bytes = __blitsenRandomBytes(16);
      bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
      bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 1
      const hex = Array.from(bytes, byte => HEX[byte]).join("");
      return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}`
        + `-${hex.slice(16, 20)}-${hex.slice(20)}`;
    },
  });

  // `TextEncoder` and `TextDecoder`, over the UTF encodings and no others. The
  // legacy single-byte tables are a genuine absence and report themselves as
  // one: an unsupported label throws, which is what the spec says and what
  // leaves feature detection able to tell.
  //
  // UTF-8 crosses to the host, which already converts it for `fetch` bodies.
  // UTF-16 is decoded here instead, because it is a walk over code units and
  // sending it out would only copy the bytes twice. Monaco's string builder
  // constructs `new TextDecoder("UTF-16LE")` with no feature test around it, so
  // an absent decoder is not a fallback there — it is an editor that renders
  // nothing.
  const ENCODINGS = new Map();
  for (const [encoding, labels] of [
    ["utf-8", ["unicode-1-1-utf-8", "unicode11utf8", "unicode20utf8", "utf-8", "utf8",
      "x-unicode20utf8"]],
    ["utf-16le", ["csunicode", "iso-10646-ucs-2", "ucs-2", "unicode", "unicodefeff", "utf-16",
      "utf-16le"]],
    ["utf-16be", ["unicodefffe", "utf-16be"]],
  ]) for (const label of labels) ENCODINGS.set(label, encoding);

  const sourceBytes = (input, caller) => {
    if (input === undefined) return new Uint8Array(0);
    if (input instanceof ArrayBuffer) return new Uint8Array(input);
    if (ArrayBuffer.isView(input)) {
      return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
    }
    throw new TypeError(`${caller} expects an ArrayBuffer or a view of one`);
  };

  // The trailing bytes of a chunk that begin a sequence it does not finish, and
  // which therefore have to wait for the next one. Only a streaming decode asks:
  // at the end of a stream an incomplete sequence is malformed rather than
  // pending, and is decoded as such.
  const heldBack = (encoding, bytes) => {
    if (encoding === "utf-8") {
      for (let back = 1; back <= 3 && back <= bytes.length; back += 1) {
        const byte = bytes[bytes.length - back];
        if (byte < 0x80) return 0;
        if (byte >= 0xc0) {
          const length = byte >= 0xf0 ? 4 : byte >= 0xe0 ? 3 : 2;
          return length > back ? back : 0;
        }
      }
      return 0;
    }
    // A half-written code unit, and then a high surrogate whose pair is in the
    // chunk that has not arrived.
    let held = bytes.length % 2;
    const last = bytes.length - held - 2;
    if (last >= 0) {
      const unit = encoding === "utf-16le"
        ? bytes[last] | (bytes[last + 1] << 8)
        : (bytes[last] << 8) | bytes[last + 1];
      if (unit >= 0xd800 && unit <= 0xdbff) held += 2;
    }
    return held;
  };

  const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;

  const decodeUtf16 = (bytes, littleEndian, fatal) => {
    // Whole code units only: a trailing odd byte is malformed input, which
    // U+FFFD stands for unless the decoder was asked to refuse it.
    const units = bytes.length >> 1;
    if (fatal && bytes.length !== units * 2) {
      throw new TypeError("invalid UTF-16: the input ends inside a code unit");
    }
    const parts = [];
    // In blocks, because `String.fromCharCode` takes its code units as
    // arguments and a large enough spread overflows the call stack.
    for (let start = 0; start < units; start += 4096) {
      const block = [];
      for (let index = start; index < Math.min(start + 4096, units); index += 1) {
        const at = index * 2;
        block.push(littleEndian
          ? bytes[at] | (bytes[at + 1] << 8)
          : (bytes[at] << 8) | bytes[at + 1]);
      }
      parts.push(String.fromCharCode(...block));
    }
    const text = parts.join("") + (bytes.length === units * 2 ? "" : "\uFFFD");
    if (fatal && LONE_SURROGATE.test(text)) {
      throw new TypeError("invalid UTF-16: the input contains an unpaired surrogate");
    }
    return text;
  };

  const decoderState = new WeakMap();
  const stateOf = (decoder, caller) => {
    const state = decoderState.get(decoder);
    if (state === undefined) {
      throw new TypeError(`${caller} called on something that is not a TextDecoder`);
    }
    return state;
  };

  class TextDecoder {
    constructor(label = "utf-8", options = {}) {
      const encoding = ENCODINGS.get(String(label).trim().toLowerCase());
      if (encoding === undefined) {
        throw new RangeError(`${label} is not an encoding Blitsen decodes`);
      }
      decoderState.set(this, {
        encoding,
        fatal: Boolean(options?.fatal),
        ignoreBOM: Boolean(options?.ignoreBOM),
        pending: new Uint8Array(0),
        started: false,
      });
    }

    get encoding() { return stateOf(this, "encoding").encoding; }
    get fatal() { return stateOf(this, "fatal").fatal; }
    get ignoreBOM() { return stateOf(this, "ignoreBOM").ignoreBOM; }

    decode(input, options = {}) {
      const state = stateOf(this, "TextDecoder.decode");
      const chunk = sourceBytes(input, "TextDecoder.decode");
      let bytes = chunk;
      if (state.pending.length > 0) {
        bytes = new Uint8Array(state.pending.length + chunk.length);
        bytes.set(state.pending);
        bytes.set(chunk, state.pending.length);
      }
      const held = options?.stream ? heldBack(state.encoding, bytes) : 0;
      state.pending = held === 0 ? new Uint8Array(0) : bytes.slice(bytes.length - held);
      const body = held === 0 ? bytes : bytes.subarray(0, bytes.length - held);
      let text = state.encoding === "utf-8"
        ? __blitsenUtf8Decode(body, state.fatal)
        : decodeUtf16(body, state.encoding === "utf-16le", state.fatal);
      // The byte-order mark belongs to the stream rather than to the text, so
      // it is dropped once, at its start. Removed from the decoded string
      // rather than from the bytes because all three encodings spell it
      // U+FEFF by the time they get here.
      if (!state.started && !state.ignoreBOM && text.startsWith("\uFEFF")) text = text.slice(1);
      if (text.length > 0) state.started = true;
      if (!options?.stream) {
        state.pending = new Uint8Array(0);
        state.started = false;
      }
      return text;
    }
  }

  class TextEncoder {
    get encoding() { return "utf-8"; }
    encode(input = "") { return __blitsenUtf8Encode(String(input)); }
  }

  define("TextDecoder", TextDecoder);
  define("TextEncoder", TextEncoder);

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

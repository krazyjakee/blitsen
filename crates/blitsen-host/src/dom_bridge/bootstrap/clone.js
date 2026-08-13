  // Structured clone, as a flat record graph.
  //
  // A message crosses a thread boundary, and the two ends are separate engines
  // that share no value representation — so the value is flattened here into
  // something the host can carry as bytes and the other end can rebuild. That
  // is why this is a graph of numbered nodes rather than a recursive walk that
  // writes JSON: a cycle, a value referenced twice, and an ArrayBuffer shared by
  // two views all have to survive the trip, and only a node table preserves
  // them.
  //
  // Binary payloads never enter the graph. They are staged with the host as
  // whole buffers and referred to by index, so bytes cross the engine boundary
  // once instead of being expanded into a JSON array of numbers.
  const TYPED_ARRAYS = {
    Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array,
    Int32Array, Uint32Array, Float32Array, Float64Array,
    BigInt64Array, BigUint64Array,
  };
  const ERROR_TYPES = { Error, EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError };
  // What a value is called in the message a DataCloneError carries. The name is
  // the whole diagnostic: "function could not be cloned" is what a developer who
  // put a callback in a message needs to read.
  const describeValue = value => {
    if (typeof value === "function") return `${value.name || "anonymous"}()`;
    if (typeof value === "symbol") return String(value);
    const tag = Object.prototype.toString.call(value).slice(8, -1);
    if (tag !== "Object") return tag;
    return value?.constructor?.name ? `${value.constructor.name} instance` : String(value);
  };
  const notCloneable = value =>
    new DOMException(`${describeValue(value)} could not be cloned.`, "DataCloneError");

  // What a message may not carry, as opposed to what it merely reshapes.
  //
  // The distinction matters more than it looks. Structured clone does *not*
  // refuse an ordinary class instance — it copies the own enumerable properties
  // and drops the prototype, so `new Point(1, 2)` arrives as `{x: 1, y: 2}`.
  // Refusing those instead would break most real senders: Monaco's editor posts
  // its protocol messages as class instances, and so does anything built on
  // Comlink. What genuinely cannot cross is a *platform* object — one whose
  // behaviour lives in the host rather than in its fields — and copying a DOM
  // node's enumerable properties would produce an empty object wearing its name,
  // which is worse than saying no.
  //
  // Resolved by name at call time rather than captured: this fragment is loaded
  // into the document's scope and a worker's, and the two do not have the same
  // platform objects in them. A name absent from a scope simply matches nothing.
  const PLATFORM_CLASSES = ["EventTarget", "NodeList", "NamedNodeMap", "DOMTokenList",
    "CSSStyleDeclaration", "CSSStyleSheet", "StyleSheetList", "CSSRule", "CSSRuleList",
    "MediaQueryList", "Navigator", "Location", "History", "Storage", "Range", "Selection",
    "Headers", "Request", "Response", "Blob", "AbortController", "MutationObserver",
    "ResizeObserver", "Promise", "WeakMap", "WeakSet", "WeakRef", "SharedArrayBuffer"];
  const isPlatformObject = value => PLATFORM_CLASSES.some(name => {
    const constructor = globalThis[name];
    return typeof constructor === "function" && value instanceof constructor;
  });

  // Numbers JSON cannot carry. `-0` is the subtle one: it round-trips through
  // JSON as `0`, and an application that divides by it would see Infinity of the
  // wrong sign on the other side of a message.
  const specialNumber = value =>
    Number.isNaN(value) ? "NaN"
      : value === Infinity ? "Infinity"
      : value === -Infinity ? "-Infinity"
      : Object.is(value, -0) ? "-0" : null;
  const SPECIAL_NUMBERS = { NaN, Infinity, "-Infinity": -Infinity, "-0": -0 };

  const encodeClone = (root, transfer = []) => {
    const nodes = [];
    const seen = new Map();
    const buffers = [];
    const ports = [];
    const transferable = new Set(transfer);
    for (const item of transferable) {
      const isBuffer = item instanceof ArrayBuffer;
      if (!isBuffer && !(item instanceof MessagePort))
        throw new DOMException(
          `${describeValue(item)} is not transferable.`, "DataCloneError");
      // `detached` is ES2024 and reads `undefined` on an engine without it,
      // which is why the comparison is explicit rather than truthy.
      if (isBuffer && item.detached === true)
        throw new DOMException("an already detached ArrayBuffer cannot be transferred.",
          "DataCloneError");
    }

    const stage = buffer => {
      buffers.push(new Uint8Array(buffer.slice(0)));
      return buffers.length - 1;
    };

    const encode = value => {
      if (seen.has(value)) return seen.get(value);
      const index = nodes.length;
      // Reserved before the children are walked, so a cycle finds this index
      // rather than recursing forever.
      const place = node => {
        nodes[index] = node;
        return index;
      };
      switch (typeof value) {
        case "undefined": return place(["u"]);
        case "boolean": return place(["b", value]);
        case "string": return place(["s", value]);
        case "bigint": return place(["i", String(value)]);
        case "number": {
          const special = specialNumber(value);
          return place(special === null ? ["d", value] : ["ds", special]);
        }
        case "symbol":
        case "function": throw notCloneable(value);
        default: break;
      }
      if (value === null) return place(["n"]);
      seen.set(value, index);
      nodes.push(null);

      if (value instanceof MessagePort) {
        if (!transferable.has(value))
          throw new DOMException("a MessagePort must be transferred, not cloned.", "DataCloneError");
        return place(["p", portIdOf(value)]);
      }
      if (value instanceof ArrayBuffer) return place(["ab", stage(value)]);
      if (ArrayBuffer.isView(value)) {
        const kind = value instanceof DataView ? "DataView" : value.constructor.name;
        if (kind !== "DataView" && !(kind in TYPED_ARRAYS)) throw notCloneable(value);
        // The view's own buffer is encoded, so two views over one buffer stay
        // two views over one buffer on the other side.
        const buffer = encode(value.buffer);
        return place(["ta", kind, buffer, value.byteOffset,
          kind === "DataView" ? value.byteLength : value.length]);
      }
      if (value instanceof Date) return place(["dt", value.getTime()]);
      if (value instanceof RegExp) return place(["re", value.source, value.flags]);
      if (value instanceof Error) {
        const name = value.name in ERROR_TYPES ? value.name : "Error";
        return place(["e", name, String(value.message), value.stack ?? ""]);
      }
      if (value instanceof Map) {
        const entries = [...value].map(([key, item]) => [encode(key), encode(item)]);
        return place(["m", entries]);
      }
      if (value instanceof Set) return place(["t", [...value].map(encode)]);
      if (value instanceof Boolean || value instanceof Number || value instanceof String)
        return place(["w", value.constructor.name, encode(value.valueOf())]);
      if (Array.isArray(value)) {
        // Recorded by index rather than as a dense list: a hole is not the same
        // value as an explicit `undefined`, and an array can carry named
        // properties as well, both of which a plain map would flatten away.
        const entries = [];
        for (const key of Object.keys(value)) entries.push([key, encode(value[key])]);
        return place(["a", value.length, entries]);
      }
      // A DOM node arriving as `{}` is a bug that surfaces far from its cause,
      // so a platform object is refused. Everything else is copied as a plain
      // object and loses its prototype, which is what structured clone does to
      // a class instance.
      if (isPlatformObject(value)) throw notCloneable(value);
      const entries = [];
      for (const key of Object.keys(value)) entries.push([key, encode(value[key])]);
      return place(["o", entries]);
    };

    const index = encode(root);
    // Detached only once the whole value has been encoded. A transfer list is
    // emptied by a *successful* send, so a message that could not be serialized
    // leaves the sender still holding everything it was going to give away.
    //
    // The list is what decides `event.ports` at the other end, not the value: a
    // port handed over without being mentioned in the message still arrives, and
    // one mentioned twice still arrives once.
    for (const item of transferable) {
      if (item instanceof MessagePort) ports.push(detachPort(item));
      else detachBuffer(item);
    }
    return { graph: JSON.stringify({ root: index, nodes }), buffers, ports };
  };

  const decodeClone = (graph, takeBuffer, adoptPort) => {
    const { root, nodes } = JSON.parse(graph);
    const built = new Array(nodes.length);
    const buffers = new Map();
    const bufferAt = index => {
      if (!buffers.has(index)) buffers.set(index, takeBuffer(index));
      return buffers.get(index);
    };
    // Three passes, because a node may refer to one that has not been read yet.
    // The first creates every value that depends on nothing, including empty
    // containers; the second builds the views, which need their buffer to exist
    // but are not themselves referred to by anything under construction; the
    // third fills the containers. A cycle therefore closes onto the same object
    // the graph recorded, and a view reaches its holder before the holder is
    // populated rather than after — which is the ordering that decides whether
    // an object carrying a `Uint8Array` arrives with one or with `undefined`.
    for (let index = 0; index < nodes.length; index++) {
      const node = nodes[index];
      switch (node[0]) {
        case "u": built[index] = undefined; break;
        case "n": built[index] = null; break;
        case "b": case "s": case "d": built[index] = node[1]; break;
        case "ds": built[index] = SPECIAL_NUMBERS[node[1]]; break;
        case "i": built[index] = BigInt(node[1]); break;
        case "dt": built[index] = new Date(node[1]); break;
        case "re": built[index] = new RegExp(node[1], node[2]); break;
        case "e": {
          const error = new ERROR_TYPES[node[1]](node[2]);
          if (node[3]) Object.defineProperty(error, "stack", { value: node[3], configurable: true, writable: true });
          built[index] = error;
          break;
        }
        case "ab": built[index] = bufferAt(node[1]).buffer; break;
        case "p": built[index] = adoptPort(node[1]); break;
        case "a": built[index] = new Array(node[1]); break;
        case "o": built[index] = {}; break;
        case "m": built[index] = new Map(); break;
        case "t": built[index] = new Set(); break;
        default: break;
      }
    }
    for (let index = 0; index < nodes.length; index++) {
      const node = nodes[index];
      if (node[0] === "ta") {
        built[index] = node[1] === "DataView"
          ? new DataView(built[node[2]], node[3], node[4])
          : new TYPED_ARRAYS[node[1]](built[node[2]], node[3], node[4]);
      } else if (node[0] === "w") {
        built[index] = new globalThis[node[1]](built[node[2]]);
      }
    }
    for (let index = 0; index < nodes.length; index++) {
      const node = nodes[index];
      switch (node[0]) {
        case "a": case "o":
          for (const [key, value] of node[node[0] === "a" ? 2 : 1]) built[index][key] = built[value];
          break;
        case "m":
          for (const [key, value] of node[1]) built[index].set(built[key], built[value]);
          break;
        case "t":
          for (const value of node[1]) built[index].add(built[value]);
          break;
        default: break;
      }
    }
    return built[root];
  };

  // The host holds a staged buffer until the decoding side asks for it, so the
  // bytes are handed over exactly once and a message that is never read does not
  // keep them alive. Staging is positional; what comes back is a token per
  // buffer, because an inbound payload outlives the call that delivered it.
  const stageBuffers = payloads => {
    for (const bytes of payloads) __blitsenCloneStage(bytes);
  };
  const detachBuffer = buffer => __blitsenDetachBuffer(buffer);
  const takeBuffers = tokens => index => __blitsenCloneTake(String(tokens[index]));

  const structuredClone = (value, options = {}) => {
    const encoded = encodeClone(value, options?.transfer ?? []);
    stageBuffers(encoded.buffers);
    // Round-tripped through the host rather than short-circuited: a clone that
    // took a different path from a posted message would be a second
    // implementation of the same operation, and the two would drift.
    const tokens = JSON.parse(__blitsenCloneAdopt());
    return decodeClone(encoded.graph, takeBuffers(tokens), portAdopter());
  };

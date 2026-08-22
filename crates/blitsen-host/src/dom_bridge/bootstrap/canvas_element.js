  // `<canvas>` itself: the element the context draws into, and the two calls
  // that ask it for an image rather than for drawing.
  //
  // Separate from the context because the element outlives it — a canvas that
  // never has `getContext` called on it is still sized, still laid out, and
  // still encodable — and because the two answer to different owners: this half
  // is HTML's, and the half above is the canvas 2D specification's.

  // A style reads back as what it was set to: a colour as a CSS string, an
  // object as itself.
  const canvasStyleValue = style => {
    if (!Array.isArray(style)) return style;
    const [red, green, blue, alpha] = style;
    const channel = value => Math.round(value * 255);
    return alpha >= 1
      ? `#${[red, green, blue].map(value =>
        channel(value).toString(16).padStart(2, "0")).join("")}`
      : `rgba(${channel(red)}, ${channel(green)}, ${channel(blue)}, ${alpha})`;
  };

  const canvasRectPath = (x, y, width, height) => {
    const path = new CanvasPath();
    path.moveTo(Number(x), Number(y));
    path.lineTo(Number(x) + Number(width), Number(y));
    path.lineTo(Number(x) + Number(width), Number(y) + Number(height));
    path.lineTo(Number(x), Number(y) + Number(height));
    path.closePath();
    return path;
  };
  const clonePath = path => {
    const copy = new CanvasPath();
    copy.tokens = path.tokens.slice();
    return copy;
  };
  // Text drawn by a canvas is one line: the specification replaces every space
  // character with U+0020 rather than breaking on it.
  const collapseCanvasText = text => String(text).replace(/[\t\n\f\r]/g, " ");
  // The intrinsic size of a `drawImage` or `createPattern` source, or null if
  // the value is not one this runtime can sample.
  const canvasSourceSize = source => {
    if (source instanceof HTMLCanvasElement) return [source.width, source.height];
    if (source instanceof HTMLImageElement) return [source.naturalWidth, source.naturalHeight];
    return null;
  };

  const canvasContexts = new WeakMap();

  class HTMLCanvasElement extends Element {
    // The backing store, which is the content attribute and not the box: a
    // canvas styled to 600 CSS pixels still reports the 300 it was given.
    get width() { return canvasSurface(this).width; }
    set width(value) { this._resize("width", value); }
    get height() { return canvasSurface(this).height; }
    set height(value) { this._resize("height", value); }
    _resize(name, value) {
      const size = Number(value);
      this.setAttribute(name, String(Number.isFinite(size) && size > 0 ? Math.floor(size) : 0));
    }
    // Writing either dimension clears the canvas and resets its context, so
    // whatever was drawn before the write has to reach the renderer before the
    // write does.
    setAttribute(name, value) {
      const dimension = String(name) === "width" || String(name) === "height";
      const context = dimension ? canvasContexts.get(this) : null;
      const before = dimension ? canvasSurface(this) : null;
      context?._settled();
      super.setAttribute(name, value);
      const after = dimension ? canvasSurface(this) : null;
      if (before && (before.width !== after.width || before.height !== after.height))
        context?._resetForResize();
    }
    removeAttribute(name) {
      const dimension = String(name) === "width" || String(name) === "height";
      const context = dimension ? canvasContexts.get(this) : null;
      context?._settled();
      super.removeAttribute(name);
      if (dimension) context?._resetForResize();
    }
    // One context per element, handed back on every call: a second
    // `getContext("2d")` is the same drawing state, not a fresh one.
    getContext(kind) {
      if (String(kind) !== "2d") return null;
      let context = canvasContexts.get(this);
      if (!context) {
        context = new CanvasRenderingContext2D(this);
        canvasContexts.set(this, context);
      }
      return context;
    }
    toDataURL(type, quality) {
      canvasContexts.get(this)?._settled();
      return __blitsenCanvasCall(String(this[handle]), "dataUrl",
        new Float64Array([Number(quality)]), [type === undefined ? "image/png" : String(type)]);
    }
    // Asynchronous because a browser's is, and application code written against
    // one expects the callback in a later task rather than inside the call.
    toBlob(callback, type, quality) {
      if (typeof callback !== "function") throw new TypeError("toBlob needs a callback");
      canvasContexts.get(this)?._settled();
      const encoded = __blitsenCanvasCall(String(this[handle]), "encode",
        new Float64Array([Number(quality)]), [type === undefined ? "image/png" : String(type)]);
      hostSetTimeout(() => callback(encoded.bytes.length === 0
        ? null : new Blob([encoded.bytes], { type: encoded.type })), 0);
    }
  }

  const canvasSurface = element => {
    const [width, height] = __blitsenCanvasCall(String(element[handle]), "size",
      new Float64Array(0), []);
    return { width, height };
  };

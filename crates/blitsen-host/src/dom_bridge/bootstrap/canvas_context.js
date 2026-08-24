  // `getContext("2d")`, and the command stream it writes.
  //
  // The whole 2D state machine is here: the transform stack, the paint styles,
  // the current path, `save`/`restore`. The renderer holds none of it — every
  // command carries the transform and the paint it is drawn with — and that is
  // the point of the split. State lives where it is read and written, which is
  // here, so a `save` costs an object copy rather than a host call; and what
  // crosses the boundary is drawing, which is the part the renderer is for.
  //
  // Commands accumulate in a `Float64Array` and are submitted in batches. A
  // canvas frame is hundreds to thousands of operations and each one would
  // otherwise be a native call; batching turns that into one call per frame.
  // The batch is flushed before the frame is painted, before anything reads
  // pixels back, and at the end of whatever task drew — never lazily enough for
  // an application to observe a stale canvas.

  const CANVAS_RESET = 0, CANVAS_FILL = 1, CANVAS_STROKE = 2, CANVAS_PUSH_CLIP = 3,
    CANVAS_PUSH_LAYER = 4, CANVAS_POP_LAYER = 5, CANVAS_TEXT = 6, CANVAS_IMAGE = 7,
    CANVAS_PUT_IMAGE = 8;
  // The order is the wire format: `blitsen-blitz`'s `canvas::wire::blend_mode`
  // holds the same list as blend modes, and the two are matched by position.
  const COMPOSITE_OPERATIONS = ["source-over", "source-in", "source-out", "source-atop",
    "destination-over", "destination-in", "destination-out", "destination-atop", "lighter",
    "copy", "xor", "multiply", "screen", "overlay", "darken", "lighten", "color-dodge",
    "color-burn", "hard-light", "soft-light", "difference", "exclusion", "hue", "saturation",
    "color", "luminosity", "plus-lighter"];
  const COMPOSITE_DESTINATION_OUT = 6;
  const LINE_CAPS = ["butt", "round", "square"];
  const LINE_JOINS = ["miter", "round", "bevel"];
  const TEXT_ALIGNMENTS = ["start", "end", "left", "right", "center"];
  const TEXT_BASELINES = ["alphabetic", "top", "hanging", "middle", "ideographic", "bottom"];
  const IMAGE_QUALITIES = ["low", "medium", "high"];
  // What `drawImage` hands `_openDrawLayer` in place of a paint style: it draws
  // pixels rather than a style, and the layer rule is the one images get.
  const IMAGE_PAINT = Symbol("Blitsen canvas image paint");

  // One batch of drawing, and the sources and strings it refers to by index.
  //
  // The numbers grow by doubling rather than through an array, because this is
  // the one structure a draw loop touches per operation and a `Float64Array`
  // that never reallocates in steady state is the difference between a canvas
  // that costs a copy per frame and one that costs a copy per command.
  class CanvasStream {
    constructor() {
      this.numbers = new Float64Array(1024);
      this.strings = [];
      this.sources = [];
      this.sourceIndices = new Map();
      this.pixels = [];
      this.reset();
    }
    reset() {
      this.length = 0;
      this.strings.length = 0;
      this.sources.length = 0;
      this.sourceIndices.clear();
      this.pixels.length = 0;
      this.pixelLength = 0;
    }
    _reserve(count) {
      if (this.length + count <= this.numbers.length) return;
      let size = this.numbers.length;
      while (size < this.length + count) size *= 2;
      const grown = new Float64Array(size);
      grown.set(this.numbers.subarray(0, this.length));
      this.numbers = grown;
    }
    put(value) {
      this._reserve(1);
      this.numbers[this.length++] = value;
    }
    putAll(values) {
      this._reserve(values.length);
      for (let index = 0; index < values.length; index++)
        this.numbers[this.length + index] = values[index];
      this.length += values.length;
    }
    text(value) {
      this.strings.push(String(value));
      this.put(this.strings.length - 1);
    }
    // An element source is keyed by the element, so a `drawImage` in a loop
    // names the same image once per batch rather than once per call.
    element(node) {
      let index = this.sourceIndices.get(node);
      if (index === undefined) {
        index = this.sources.length;
        this.sourceIndices.set(node, index);
        this.sources.push([1, node]);
      }
      return index;
    }
    imageData(image) {
      const index = this.sources.length;
      this.sources.push([0, image.width, image.height, this.pixelLength]);
      this.pixels.push(image.data);
      this.pixelLength += image.data.length;
      return index;
    }
    // The submitted buffer is the source preamble followed by the commands.
    // Built here rather than reserved at the head, because how many sources a
    // batch names is only known once it is over.
    build() {
      const preamble = [this.sources.length];
      for (const source of this.sources) preamble.push(...source);
      const numbers = new Float64Array(preamble.length + this.length);
      numbers.set(preamble);
      numbers.set(this.numbers.subarray(0, this.length), preamble.length);
      const pixels = new Uint8ClampedArray(this.pixelLength);
      let offset = 0;
      for (const chunk of this.pixels) { pixels.set(chunk, offset); offset += chunk.length; }
      return [numbers, this.strings, pixels];
    }
  }

  // Contexts with commands the renderer has not seen yet.
  const drawnCanvases = new Set();
  const flushCanvases = () => {
    for (const context of drawnCanvases) context._flush();
    drawnCanvases.clear();
  };
  // A draw outside `requestAnimationFrame` — in a load handler, a timer, an
  // event listener — still has to reach the screen. Scheduling the flush as a
  // microtask submits it before control leaves the task that drew, and the
  // frame boundary flushes again for anything drawn inside a frame callback.
  let canvasFlushScheduled = false;
  const scheduleCanvasFlush = () => {
    if (canvasFlushScheduled) return;
    canvasFlushScheduled = true;
    Promise.resolve().then(() => { canvasFlushScheduled = false; flushCanvases(); });
  };
  // Whether a canvas has been drawn on since the last frame was painted. The
  // host stops turning when nothing is pending, so without this a canvas drawn
  // outside a frame callback would wait for the next unrelated reason to paint.
  let canvasPaintPending = false;

  const defaultCanvasState = () => ({
    transform: IDENTITY_MATRIX,
    fillStyle: [0, 0, 0, 1],
    strokeStyle: [0, 0, 0, 1],
    globalAlpha: 1,
    composite: 0,
    lineWidth: 1,
    lineCap: 0,
    lineJoin: 0,
    miterLimit: 10,
    lineDash: [],
    lineDashOffset: 0,
    font: { style: 0, weight: 400, stretch: 100, size: 10, families: "sans-serif" },
    textAlign: 0,
    textBaseline: 0,
    rtl: false,
    imageSmoothingEnabled: true,
    imageSmoothingQuality: 0,
    clipDepth: 0,
  });

  class CanvasRenderingContext2D {
    constructor(element) {
      this._element = element;
      this._stream = new CanvasStream();
      this._size = null;
      this._state = defaultCanvasState();
      this._saved = [];
      // Clips are a layer in the recorded scene, and a batch must close every
      // layer it opens — so the stack is what is *wanted* and `_appliedClips`
      // is how much of it the current batch has opened.
      this._clips = [];
      this._appliedClips = 0;
      this._path = new CanvasPath();
    }

    get canvas() { return this._element; }

    // --- the batch ------------------------------------------------------

    _call(operation, numbers = [], strings = [], pixels = null) {
      return __blitsenCanvasCall(String(this._element[handle]), operation,
        numbers instanceof Float64Array ? numbers : new Float64Array(numbers), strings, pixels);
    }
    // Marks the batch as owed to the renderer, without touching the layers.
    _touch() {
      drawnCanvases.add(this);
      canvasPaintPending = true;
      scheduleCanvasFlush();
    }
    _begin() {
      this._touch();
      while (this._appliedClips < this._clips.length) {
        const clip = this._clips[this._appliedClips++];
        this._stream.put(CANVAS_PUSH_CLIP);
        this._stream.putAll(clip.transform);
        this._writePath(clip.path);
      }
    }
    _closeClips() {
      for (let depth = 0; depth < this._appliedClips; depth++)
        this._stream.put(CANVAS_POP_LAYER);
      this._appliedClips = 0;
    }
    _flush() {
      this._size = null;
      if (this._stream.length === 0 && this._appliedClips === 0) return;
      this._closeClips();
      const [numbers, strings, pixels] = this._stream.build();
      try {
        this._call("submit", numbers, strings, pixels);
      } finally {
        // `build` returns the stream's strings array, so the synchronous bridge
        // must consume it before reset clears that storage for the next batch.
        this._stream.reset();
      }
    }
    // The backing store size, read once per batch. `clearRect` and every
    // composited draw need it, and a draw loop would otherwise ask the renderer
    // for a number that cannot change while the batch is open.
    _backingStore() {
      return (this._size ??= canvasSurface(this._element));
    }
    // Everything that reads the canvas back has to see what was drawn into it.
    _settled() {
      this._flush();
      drawnCanvases.delete(this);
      return this;
    }

    // --- writing one command --------------------------------------------

    _writePath(path) {
      this._stream.put(path.tokens.length);
      this._stream.putAll(path.tokens);
    }
    _writeColor(rgba, alpha) {
      this._stream.putAll([rgba[0], rgba[1], rgba[2], rgba[3] * alpha]);
    }
    // `globalAlpha` is folded into the paint rather than carried as a layer
    // opacity: for a single shape the two are the same result, and a layer is
    // a full-canvas composite the renderer would otherwise pay for per draw.
    _writePaint(style, alpha) {
      if (Array.isArray(style)) {
        this._stream.put(0);
        this._writeColor(style, alpha);
        return;
      }
      if (style instanceof CanvasGradient) {
        this._stream.put(1);
        this._stream.put(style._kind);
        this._stream.putAll(style._geometry);
        this._stream.put(style._stops.length);
        for (const [offset, rgba] of style._stops) {
          this._stream.put(offset);
          this._writeColor(rgba, alpha);
        }
        return;
      }
      const index = this._stream.element(style._source[handle]);
      this._stream.put(2);
      this._stream.put(index);
      this._stream.putAll([style._extends[0] ? 1 : 0, style._extends[1] ? 1 : 0,
        this._state.imageSmoothingEnabled ? this._state.imageSmoothingQuality : 0]);
      this._stream.putAll(style._transform);
    }
    _writeStroke() {
      const state = this._state;
      this._stream.putAll([state.lineWidth, state.lineCap, state.lineJoin, state.miterLimit,
        state.lineDashOffset, state.lineDash.length]);
      this._stream.putAll(state.lineDash);
    }
    // Opens whatever layer this draw needs, and reports the opacity its paint
    // should carry.
    //
    // Two reasons a draw becomes a layer. A composite operation other than
    // `source-over` is a layer over the whole canvas because that is its
    // scope — `source-in` clears the canvas everywhere the shape being drawn is
    // not. And an *image* paint at less than full opacity is a layer because
    // the renderer cannot apply an opacity to an image sampler at all: it
    // refuses one rather than approximating it, and a canvas that asked would
    // take the process down. A colour or a gradient has no such trouble, and
    // folds `globalAlpha` into its own components for one composite pass fewer.
    _openDrawLayer(style) {
      const { composite, globalAlpha } = this._state;
      const image = style !== undefined && !Array.isArray(style)
        && !(style instanceof CanvasGradient);
      const layerAlpha = image ? globalAlpha : 1;
      const paintAlpha = image ? 1 : globalAlpha;
      if (composite === 0 && layerAlpha === 1) return [false, paintAlpha];
      const { width, height } = this._backingStore();
      this._pushLayer(composite, layerAlpha, IDENTITY_MATRIX,
        canvasRectPath(0, 0, width, height));
      return [true, paintAlpha];
    }
    _pushLayer(blend, alpha, transform, path) {
      this._stream.put(CANVAS_PUSH_LAYER);
      this._stream.put(blend);
      this._stream.put(alpha);
      this._stream.putAll(transform);
      this._writePath(path);
    }

    _fillPath(path, evenOdd) {
      this._begin();
      const [composited, alpha] = this._openDrawLayer(this._state.fillStyle);
      this._stream.put(CANVAS_FILL);
      this._writePaint(this._state.fillStyle, alpha);
      this._stream.put(evenOdd ? 1 : 0);
      this._stream.putAll(this._state.transform);
      this._writePath(path);
      if (composited) this._stream.put(CANVAS_POP_LAYER);
    }
    _strokePath(path) {
      this._begin();
      const [composited, alpha] = this._openDrawLayer(this._state.strokeStyle);
      this._stream.put(CANVAS_STROKE);
      this._writePaint(this._state.strokeStyle, alpha);
      this._writeStroke();
      this._stream.putAll(this._state.transform);
      this._writePath(path);
      if (composited) this._stream.put(CANVAS_POP_LAYER);
    }

    // --- state ----------------------------------------------------------

    save() {
      this._saved.push({ ...this._state, lineDash: this._state.lineDash.slice(),
        clipDepth: this._clips.length });
    }
    restore() {
      const state = this._saved.pop();
      if (!state) return;
      // A clip cannot be lifted from a recorded layer, so the layers opened
      // since the matching `save` are closed and the stack is truncated to what
      // that `save` saw.
      if (this._appliedClips > state.clipDepth) {
        this._touch();
        for (let depth = state.clipDepth; depth < this._appliedClips; depth++)
          this._stream.put(CANVAS_POP_LAYER);
        this._appliedClips = state.clipDepth;
      }
      this._clips.length = state.clipDepth;
      this._state = state;
    }
    reset() {
      this._touch();
      // Both the layers this batch opened and the drawing under them go, so
      // there is nothing left to close: `RESET` is the whole batch.
      this._stream.reset();
      this._appliedClips = 0;
      this._stream.put(CANVAS_RESET);
      this._clips = [];
      this._state = defaultCanvasState();
      this._saved = [];
      this._path = new CanvasPath();
    }
    // What `canvas.width = …` does to the context: the state goes back to its
    // defaults and the batch is dropped, because the renderer has already
    // cleared the backing store the batch was going to draw into.
    _resetForResize() {
      this._stream.reset();
      this._size = null;
      this._appliedClips = 0;
      this._clips = [];
      this._state = defaultCanvasState();
      this._saved = [];
      this._path = new CanvasPath();
      drawnCanvases.delete(this);
    }

    scale(x, y) { this._multiply([Number(x), 0, 0, Number(y), 0, 0]); }
    translate(x, y) { this._multiply([1, 0, 0, 1, Number(x), Number(y)]); }
    rotate(radians) {
      const angle = Number(radians);
      this._multiply([Math.cos(angle), Math.sin(angle), -Math.sin(angle), Math.cos(angle), 0, 0]);
    }
    transform(a, b, c, d, e, f) { this._multiply([a, b, c, d, e, f].map(Number)); }
    _multiply(matrix) {
      if (!finiteNumbers(matrix)) return;
      this._state.transform = multiplyMatrix(this._state.transform, matrix);
    }
    setTransform(a, b, c, d, e, f) {
      const matrix = a !== undefined && typeof a === "object" ? matrixOf(a)
        : [a ?? 1, b ?? 0, c ?? 0, d ?? 1, e ?? 0, f ?? 0].map(Number);
      if (finiteNumbers(matrix)) this._state.transform = matrix;
    }
    resetTransform() { this._state.transform = IDENTITY_MATRIX; }
    getTransform() { return new DOMMatrix(this._state.transform); }

    get globalAlpha() { return this._state.globalAlpha; }
    set globalAlpha(value) {
      const alpha = Number(value);
      if (alpha >= 0 && alpha <= 1) this._state.globalAlpha = alpha;
    }
    get globalCompositeOperation() { return COMPOSITE_OPERATIONS[this._state.composite]; }
    set globalCompositeOperation(value) {
      const index = COMPOSITE_OPERATIONS.indexOf(String(value));
      if (index >= 0) this._state.composite = index;
    }
    get fillStyle() { return canvasStyleValue(this._state.fillStyle); }
    set fillStyle(value) { this._setStyle("fillStyle", value); }
    get strokeStyle() { return canvasStyleValue(this._state.strokeStyle); }
    set strokeStyle(value) { this._setStyle("strokeStyle", value); }
    _setStyle(slot, value) {
      if (value instanceof CanvasGradient || value instanceof CanvasPattern) {
        this._state[slot] = value;
        return;
      }
      const rgba = parseCanvasColor(value);
      // An unparseable colour is ignored rather than refused, which is what the
      // specification says and what every canvas that writes a CSS variable
      // into a fill style depends on.
      if (rgba) this._state[slot] = rgba;
    }

    get lineWidth() { return this._state.lineWidth; }
    set lineWidth(value) {
      const width = Number(value);
      if (width > 0 && Number.isFinite(width)) this._state.lineWidth = width;
    }
    get lineCap() { return LINE_CAPS[this._state.lineCap]; }
    set lineCap(value) {
      const index = LINE_CAPS.indexOf(String(value));
      if (index >= 0) this._state.lineCap = index;
    }
    get lineJoin() { return LINE_JOINS[this._state.lineJoin]; }
    set lineJoin(value) {
      const index = LINE_JOINS.indexOf(String(value));
      if (index >= 0) this._state.lineJoin = index;
    }
    get miterLimit() { return this._state.miterLimit; }
    set miterLimit(value) {
      const limit = Number(value);
      if (limit > 0 && Number.isFinite(limit)) this._state.miterLimit = limit;
    }
    getLineDash() { return this._state.lineDash.slice(); }
    setLineDash(segments) {
      const values = Array.from(segments ?? [], Number);
      if (!finiteNumbers(values) || values.some(value => value < 0)) return;
      // An odd-length pattern repeats to an even one, which is what makes
      // `[5]` mean five on, five off.
      this._state.lineDash = values.length % 2 ? values.concat(values) : values;
    }
    get lineDashOffset() { return this._state.lineDashOffset; }
    set lineDashOffset(value) {
      const offset = Number(value);
      if (Number.isFinite(offset)) this._state.lineDashOffset = offset;
    }

    get font() { return serializeCanvasFont(this._state.font); }
    set font(value) {
      const font = parseCanvasFont(value);
      if (font) this._state.font = font;
    }
    get textAlign() { return TEXT_ALIGNMENTS[this._state.textAlign]; }
    set textAlign(value) {
      const index = TEXT_ALIGNMENTS.indexOf(String(value));
      if (index >= 0) this._state.textAlign = index;
    }
    get textBaseline() { return TEXT_BASELINES[this._state.textBaseline]; }
    set textBaseline(value) {
      const index = TEXT_BASELINES.indexOf(String(value));
      if (index >= 0) this._state.textBaseline = index;
    }
    get direction() { return this._state.rtl ? "rtl" : "ltr"; }
    set direction(value) {
      // `inherit` resolves to the document's direction, and this runtime's
      // documents are left-to-right unless the element says otherwise.
      if (value === "rtl" || value === "ltr") this._state.rtl = value === "rtl";
      else if (value === "inherit") this._state.rtl = false;
    }
    get imageSmoothingEnabled() { return this._state.imageSmoothingEnabled; }
    set imageSmoothingEnabled(value) { this._state.imageSmoothingEnabled = Boolean(value); }
    get imageSmoothingQuality() { return IMAGE_QUALITIES[this._state.imageSmoothingQuality]; }
    set imageSmoothingQuality(value) {
      const index = IMAGE_QUALITIES.indexOf(String(value));
      if (index >= 0) this._state.imageSmoothingQuality = index;
    }

    // --- paths ----------------------------------------------------------

    beginPath() { this._path = new CanvasPath(); }
    closePath() { this._path.closePath(); }
    moveTo(x, y) { this._path.moveTo(x, y); }
    lineTo(x, y) { this._path.lineTo(x, y); }
    quadraticCurveTo(cx, cy, x, y) { this._path.quadraticCurveTo(cx, cy, x, y); }
    bezierCurveTo(c1x, c1y, c2x, c2y, x, y) {
      this._path.bezierCurveTo(c1x, c1y, c2x, c2y, x, y);
    }
    arc(x, y, radius, start, end, counterclockwise) {
      this._path.arc(x, y, radius, start, end, counterclockwise);
    }
    arcTo(x1, y1, x2, y2, radius) { this._path.arcTo(x1, y1, x2, y2, radius); }
    ellipse(x, y, rx, ry, rotation, start, end, counterclockwise) {
      this._path.ellipse(x, y, rx, ry, rotation, start, end, counterclockwise);
    }
    rect(x, y, width, height) { this._path.rect(x, y, width, height); }
    roundRect(x, y, width, height, radii) { this._path.roundRect(x, y, width, height, radii); }

    // The path-or-rule overload every path-consuming method carries.
    _resolvePath(first, second) {
      if (first instanceof Path2D) return [first, second === "evenodd"];
      return [this._path, first === "evenodd"];
    }
    fill(first, second) {
      const [path, evenOdd] = this._resolvePath(first, second);
      this._fillPath(path, evenOdd);
    }
    stroke(path) { this._strokePath(path instanceof Path2D ? path : this._path); }
    clip(first, second) {
      const [path] = this._resolvePath(first, second);
      // Recorded rather than applied: a clip only means anything to the
      // commands after it, and the layer is opened when the next one is written.
      this._clips.push({ transform: this._state.transform, path: clonePath(path) });
    }
    isPointInPath(first, second, third, fourth) {
      const path = first instanceof Path2D ? first : this._path;
      const [x, y] = first instanceof Path2D ? [second, third] : [first, second];
      const rule = first instanceof Path2D ? fourth : third;
      return this._contains(false, path, x, y, rule === "evenodd");
    }
    isPointInStroke(first, second, third) {
      const path = first instanceof Path2D ? first : this._path;
      const [x, y] = first instanceof Path2D ? [second, third] : [first, second];
      return this._contains(true, path, x, y, false);
    }
    _contains(stroked, path, x, y, evenOdd) {
      const numbers = [stroked ? 1 : 0, evenOdd ? 1 : 0];
      if (stroked) {
        const state = this._state;
        numbers.push(state.lineWidth, state.lineCap, state.lineJoin, state.miterLimit,
          state.lineDashOffset, state.lineDash.length, ...state.lineDash);
      }
      numbers.push(...this._state.transform, path.tokens.length, ...path.tokens,
        Number(x), Number(y));
      return this._call("contains", numbers);
    }

    // --- rectangles ------------------------------------------------------

    fillRect(x, y, width, height) {
      if (!finiteNumbers([x, y, width, height].map(Number))) return;
      this._fillPath(canvasRectPath(x, y, width, height), false);
    }
    strokeRect(x, y, width, height) {
      if (!finiteNumbers([x, y, width, height].map(Number))) return;
      this._strokePath(canvasRectPath(x, y, width, height));
    }
    clearRect(x, y, width, height) {
      const values = [x, y, width, height].map(Number);
      if (!finiteNumbers(values) || values[2] === 0 || values[3] === 0) return;
      // A clear that covers the whole backing store with nothing clipping it is
      // the start of most canvas frames. Recording it as "forget everything"
      // rather than as an erasing layer is what keeps a scene from growing by a
      // layer per frame for the life of the application.
      if (this._appliedClips === 0 && this._clips.length === 0 && this._coversCanvas(values)) {
        this._touch();
        this._stream.reset();
        this._stream.put(CANVAS_RESET);
        return;
      }
      this._begin();
      const path = canvasRectPath(...values);
      this._pushLayer(COMPOSITE_DESTINATION_OUT, 1, this._state.transform, path);
      this._stream.put(CANVAS_FILL);
      this._stream.putAll([0, 0, 0, 0, 1]);
      this._stream.put(0);
      this._stream.putAll(this._state.transform);
      this._writePath(path);
      this._stream.put(CANVAS_POP_LAYER);
    }
    _coversCanvas([x, y, width, height]) {
      const [a, b, c, d, e, f] = this._state.transform;
      // Only an axis-aligned transform can be checked this cheaply, and every
      // other one falls through to the erasing layer rather than guessing.
      if (b !== 0 || c !== 0) return false;
      const { width: canvasWidth, height: canvasHeight } = this._backingStore();
      const [x0, x1] = [a * x + e, a * (x + width) + e].sort((left, right) => left - right);
      const [y0, y1] = [d * y + f, d * (y + height) + f].sort((left, right) => left - right);
      return x0 <= 0 && y0 <= 0 && x1 >= canvasWidth && y1 >= canvasHeight;
    }

    // --- text -------------------------------------------------------------

    fillText(text, x, y, maxWidth) { this._text(text, x, y, maxWidth, false); }
    strokeText(text, x, y, maxWidth) { this._text(text, x, y, maxWidth, true); }
    _text(text, x, y, maxWidth, stroked) {
      const values = [Number(x), Number(y)];
      if (!finiteNumbers(values)) return;
      const state = this._state;
      const style = stroked ? state.strokeStyle : state.fillStyle;
      this._begin();
      const [composited, alpha] = this._openDrawLayer(style);
      this._stream.put(CANVAS_TEXT);
      this._writePaint(style, alpha);
      this._stream.put(stroked ? 1 : 0);
      if (stroked) this._writeStroke();
      this._stream.putAll(state.transform);
      this._stream.text(state.font.families);
      this._stream.putAll([state.font.size, state.font.weight, state.font.style,
        state.font.stretch, state.textAlign, state.textBaseline, state.rtl ? 1 : 0]);
      const limit = maxWidth === undefined ? 0 : Number(maxWidth);
      this._stream.putAll([values[0], values[1], limit > 0 ? limit : 0]);
      this._stream.text(collapseCanvasText(text));
      if (composited) this._stream.put(CANVAS_POP_LAYER);
    }
    measureText(text) {
      const state = this._state;
      return new TextMetrics(this._call("measure",
        [state.font.size, state.font.weight, state.font.style, state.font.stretch,
          state.textAlign, state.textBaseline, state.rtl ? 1 : 0],
        [state.font.families, collapseCanvasText(text)]));
    }

    // --- images -----------------------------------------------------------

    drawImage(source, ...rest) {
      const size = canvasSourceSize(source);
      if (!size) throw new TypeError("drawImage source must be an image or a canvas");
      const [sourceWidth, sourceHeight] = size;
      // Three, five and nine arguments: the same command with the source
      // rectangle and the destination rectangle filled in by different rules.
      const numbers = rest.map(Number);
      const [sx, sy, sw, sh, dx, dy, dw, dh] = numbers.length >= 8
        ? numbers
        : [0, 0, sourceWidth, sourceHeight, numbers[0], numbers[1],
          numbers.length >= 4 ? numbers[2] : sourceWidth,
          numbers.length >= 4 ? numbers[3] : sourceHeight];
      if (!finiteNumbers([sx, sy, sw, sh, dx, dy, dw, dh])) return;
      if (sw === 0 || sh === 0 || dw === 0 || dh === 0) return;
      // A source with nothing decoded in it draws nothing, rather than drawing
      // a placeholder or throwing.
      if (sourceWidth === 0 || sourceHeight === 0) return;
      const index = this._stream.element(source[handle]);
      this._begin();
      // `IMAGE_PAINT` rather than a style: what is drawn here is pixels, so
      // `globalAlpha` becomes a layer for the reason `_openDrawLayer` gives.
      const [composited] = this._openDrawLayer(IMAGE_PAINT);
      this._stream.put(CANVAS_IMAGE);
      this._stream.put(index);
      this._stream.put(this._state.imageSmoothingEnabled
        ? this._state.imageSmoothingQuality : 0);
      this._stream.putAll(this._state.transform);
      this._stream.putAll([sx, sy, sw, sh, dx, dy, dw, dh]);
      if (composited) this._stream.put(CANVAS_POP_LAYER);
    }

    createLinearGradient(x0, y0, x1, y1) {
      const values = [x0, y0, x1, y1].map(Number);
      if (!finiteNumbers(values)) throw new TypeError("gradient coordinates must be finite");
      return new CanvasGradient(0, values);
    }
    createRadialGradient(x0, y0, r0, x1, y1, r1) {
      const values = [x0, y0, r0, x1, y1, r1].map(Number);
      if (!finiteNumbers(values)) throw new TypeError("gradient coordinates must be finite");
      if (values[2] < 0 || values[5] < 0)
        throw new DOMException("Gradient radii must be non-negative", "IndexSizeError");
      return new CanvasGradient(1, values);
    }
    createConicGradient(startAngle, x, y) {
      const values = [x, y, startAngle].map(Number);
      if (!finiteNumbers(values)) throw new TypeError("gradient coordinates must be finite");
      return new CanvasGradient(2, [values[0], values[1], values[2],
        values[2] + Math.PI * 2]);
    }
    createPattern(source, repetition) {
      if (!canvasSourceSize(source))
        throw new TypeError("createPattern source must be an image or a canvas");
      const mode = repetition === null || repetition === undefined || repetition === ""
        ? "repeat" : String(repetition);
      if (!PATTERN_EXTENDS[mode])
        throw new DOMException(`Unknown pattern repetition "${mode}"`, "SyntaxError");
      return new CanvasPattern(source, PATTERN_EXTENDS[mode]);
    }

    createImageData(first, second) {
      if (first instanceof ImageData) return new ImageData(first.width, first.height);
      return new ImageData(Math.abs(Number(first)) | 0, Math.abs(Number(second)) | 0);
    }
    getImageData(x, y, width, height) {
      const values = [x, y, width, height].map(Number);
      if (!finiteNumbers(values))
        throw new TypeError("getImageData needs a finite rectangle");
      if (values[2] === 0 || values[3] === 0)
        throw new DOMException("getImageData needs a non-empty rectangle", "IndexSizeError");
      // A negative extent is the same rectangle written from the other corner.
      const left = values[2] < 0 ? values[0] + values[2] : values[0];
      const top = values[3] < 0 ? values[1] + values[3] : values[1];
      const [w, h] = [Math.abs(values[2]) | 0, Math.abs(values[3]) | 0];
      const pixels = this._settled()._call("pixels", [left, top, w, h]);
      return new ImageData(pixels, w, h);
    }
    putImageData(image, x, y, dirtyX, dirtyY, dirtyWidth, dirtyHeight) {
      if (!(image instanceof ImageData))
        throw new TypeError("putImageData needs an ImageData");
      const [originX, originY] = [Number(x), Number(y)];
      if (!finiteNumbers([originX, originY]))
        throw new TypeError("putImageData needs a finite destination");
      let [left, top, width, height] = dirtyWidth === undefined
        ? [0, 0, image.width, image.height]
        : [Number(dirtyX), Number(dirtyY), Number(dirtyWidth), Number(dirtyHeight)];
      if (!finiteNumbers([left, top, width, height])) return;
      if (width < 0) { left += width; width = -width; }
      if (height < 0) { top += height; height = -height; }
      left = Math.max(0, left);
      top = Math.max(0, top);
      width = Math.min(width, image.width - left);
      height = Math.min(height, image.height - top);
      if (width <= 0 || height <= 0) return;
      // Neither the transform nor the clip reaches `putImageData`, so the batch
      // closes its clip layers first and reopens them for whatever draws next.
      // `_touch` rather than `_begin` for exactly that: opening them here only
      // to close them again is two commands that cancel out.
      this._touch();
      this._closeClips();
      const index = this._stream.imageData(image);
      this._stream.put(CANVAS_PUT_IMAGE);
      this._stream.put(index);
      this._stream.putAll([originX, originY,
        originX + left, originY + top, width, height]);
    }

    // Absent by decision rather than omission: shadows and `filter` both need a
    // blur, and nothing under this runtime has one — the same reason
    // `doctor` reports CSS `filter` as ignored. See COMPATIBILITY.md.
  }

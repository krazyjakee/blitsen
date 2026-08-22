  // The 2D context's value objects, and the three grammars it is configured
  // through: a CSS colour, the `font` shorthand, and SVG path data.
  //
  // All three are parsed here rather than in the renderer, and for one reason:
  // an unparseable value is *ignored* by the specification — `ctx.fillStyle =
  // "notacolour"` leaves the previous colour in place and throws nothing — so
  // the parse has to answer "no" cheaply and often. A bridge call per assignment
  // would put a host boundary inside a draw loop to be told nothing changed.
  //
  // What crosses the boundary instead is the parsed result: four numbers for a
  // colour, a family list and a size for a font, a token stream for a path. The
  // renderer never sees a CSS string from a canvas.

  // Every colour keyword CSS defines, as one string rather than 148 object
  // entries: the table is looked up by name and never enumerated, and this
  // form costs a `split` once instead of an object literal in every context.
  const CANVAS_COLOR_KEYWORDS =
    "aliceblue f0f8ff antiquewhite faebd7 aqua 00ffff aquamarine 7fffd4 azure f0ffff beige f5f5dc " +
    "bisque ffe4c4 black 000000 blanchedalmond ffebcd blue 0000ff blueviolet 8a2be2 brown a52a2a " +
    "burlywood deb887 cadetblue 5f9ea0 chartreuse 7fff00 chocolate d2691e coral ff7f50 " +
    "cornflowerblue 6495ed cornsilk fff8dc crimson dc143c cyan 00ffff darkblue 00008b " +
    "darkcyan 008b8b darkgoldenrod b8860b darkgray a9a9a9 darkgreen 006400 darkgrey a9a9a9 " +
    "darkkhaki bdb76b darkmagenta 8b008b darkolivegreen 556b2f darkorange ff8c00 " +
    "darkorchid 9932cc darkred 8b0000 darksalmon e9967a darkseagreen 8fbc8f " +
    "darkslateblue 483d8b darkslategray 2f4f4f darkslategrey 2f4f4f darkturquoise 00ced1 " +
    "darkviolet 9400d3 deeppink ff1493 deepskyblue 00bfff dimgray 696969 dimgrey 696969 " +
    "dodgerblue 1e90ff firebrick b22222 floralwhite fffaf0 forestgreen 228b22 fuchsia ff00ff " +
    "gainsboro dcdcdc ghostwhite f8f8ff gold ffd700 goldenrod daa520 gray 808080 green 008000 " +
    "greenyellow adff2f grey 808080 honeydew f0fff0 hotpink ff69b4 indianred cd5c5c " +
    "indigo 4b0082 ivory fffff0 khaki f0e68c lavender e6e6fa lavenderblush fff0f5 " +
    "lawngreen 7cfc00 lemonchiffon fffacd lightblue add8e6 lightcoral f08080 lightcyan e0ffff " +
    "lightgoldenrodyellow fafad2 lightgray d3d3d3 lightgreen 90ee90 lightgrey d3d3d3 " +
    "lightpink ffb6c1 lightsalmon ffa07a lightseagreen 20b2aa lightskyblue 87cefa " +
    "lightslategray 778899 lightslategrey 778899 lightsteelblue b0c4de lightyellow ffffe0 " +
    "lime 00ff00 limegreen 32cd32 linen faf0e6 magenta ff00ff maroon 800000 " +
    "mediumaquamarine 66cdaa mediumblue 0000cd mediumorchid ba55d3 mediumpurple 9370db " +
    "mediumseagreen 3cb371 mediumslateblue 7b68ee mediumspringgreen 00fa9a " +
    "mediumturquoise 48d1cc mediumvioletred c71585 midnightblue 191970 mintcream f5fffa " +
    "mistyrose ffe4e1 moccasin ffe4b5 navajowhite ffdead navy 000080 oldlace fdf5e6 " +
    "olive 808000 olivedrab 6b8e23 orange ffa500 orangered ff4500 orchid da70d6 " +
    "palegoldenrod eee8aa palegreen 98fb98 paleturquoise afeeee palevioletred db7093 " +
    "papayawhip ffefd5 peachpuff ffdab9 peru cd853f pink ffc0cb plum dda0dd powderblue b0e0e6 " +
    "purple 800080 rebeccapurple 663399 red ff0000 rosybrown bc8f8f royalblue 4169e1 " +
    "saddlebrown 8b4513 salmon fa8072 sandybrown f4a460 seagreen 2e8b57 seashell fff5ee " +
    "sienna a0522d silver c0c0c0 skyblue 87ceeb slateblue 6a5acd slategray 708090 " +
    "slategrey 708090 snow fffafa springgreen 00ff7f steelblue 4682b4 tan d2b48c teal 008080 " +
    "thistle d8bfd8 tomato ff6347 turquoise 40e0d0 violet ee82ee wheat f5deb3 white ffffff " +
    "whitesmoke f5f5f5 yellow ffff00 yellowgreen 9acd32";
  let canvasColorKeywords = null;
  const canvasColorKeyword = name => {
    if (!canvasColorKeywords) {
      canvasColorKeywords = new Map();
      const parts = CANVAS_COLOR_KEYWORDS.split(" ");
      for (let index = 0; index < parts.length; index += 2)
        canvasColorKeywords.set(parts[index], parts[index + 1]);
    }
    return canvasColorKeywords.get(name);
  };

  const clamp01 = value => (value < 0 ? 0 : value > 1 ? 1 : value);
  // A component written either as a number in one range or as a percentage.
  const colorComponent = (token, scale) => {
    if (token === "none") return 0;
    const percent = token.endsWith("%");
    const value = Number.parseFloat(percent ? token.slice(0, -1) : token);
    if (!Number.isFinite(value)) return null;
    return clamp01(percent ? value / 100 : value / scale);
  };
  const hueDegrees = token => {
    const value = Number.parseFloat(token);
    if (!Number.isFinite(value)) return null;
    const unit = /[a-z]+$/.exec(token)?.[0];
    const turns = { deg: 360, grad: 400, rad: 2 * Math.PI, turn: 1 }[unit ?? "deg"];
    return turns === undefined ? null : (((value / turns) % 1) + 1) % 1;
  };
  // HSL to RGB, in the form the CSS Color specification writes it.
  const fromHsl = (hue, saturation, lightness) => {
    const channel = offset => {
      const shifted = (offset / 30 + hue * 12) % 12;
      const amount = saturation * Math.min(lightness, 1 - lightness);
      return lightness - amount * Math.max(-1, Math.min(shifted - 3, 9 - shifted, 1));
    };
    return [channel(0), channel(240), channel(120)];
  };

  // Parses a CSS colour into four components in the 0–1 range, or null.
  //
  // Hex, `rgb()`, `hsl()`, `hwb()` and the keywords: the syntaxes a canvas is
  // actually configured with. The Color 4 colour spaces — `lab()`, `oklch()`,
  // `color()` — are deliberately absent, and an assignment naming one is
  // ignored exactly as an assignment naming a misspelt keyword is.
  const parseCanvasColor = value => {
    const text = String(value).trim().toLowerCase();
    if (text === "transparent") return [0, 0, 0, 0];
    const keyword = canvasColorKeyword(text);
    const hex = keyword ?? (text.startsWith("#") ? text.slice(1) : null);
    if (hex !== null) {
      if (!/^[0-9a-f]+$/.test(hex)) return null;
      const wide = hex.length === 6 || hex.length === 8;
      if (!wide && hex.length !== 3 && hex.length !== 4) return null;
      const step = wide ? 2 : 1;
      const channel = index => {
        const part = hex.slice(index * step, index * step + step);
        return Number.parseInt(wide ? part : part + part, 16) / 255;
      };
      return [channel(0), channel(1), channel(2), hex.length % 4 === 0 ? channel(3) : 1];
    }
    const call = /^(rgba?|hsla?|hwb)\(([^)]*)\)$/.exec(text);
    if (!call) return null;
    // Both the legacy comma syntax and the modern space syntax, with `/` for
    // the alpha in the second. Splitting on either is enough to tell them
    // apart, because no component may contain a comma or a slash.
    const parts = call[2].replace(/\//g, " / ").split(/[\s,]+/).filter(Boolean);
    const slash = parts.indexOf("/");
    const alphaToken = slash >= 0 ? parts[slash + 1] : parts[3];
    const components = slash >= 0 ? parts.slice(0, slash) : parts.slice(0, 3);
    if (components.length !== 3) return null;
    const alpha = alphaToken === undefined ? 1 : colorComponent(alphaToken, 1);
    if (alpha === null) return null;
    if (call[1].startsWith("rgb")) {
      const rgb = components.map(token => colorComponent(token, 255));
      return rgb.some(value => value === null) ? null : [...rgb, alpha];
    }
    const hue = hueDegrees(components[0]);
    const first = colorComponent(components[1], 100);
    const second = colorComponent(components[2], 100);
    if (hue === null || first === null || second === null) return null;
    if (call[1] === "hwb") {
      // White and black are applied to a fully saturated hue, which is what
      // `hwb()` means: the hue's pure colour, then washed and darkened.
      const scale = first + second > 1 ? 1 / (first + second) : 1;
      const [white, black] = [first * scale, second * scale];
      return [...fromHsl(hue, 1, 0.5).map(value => value * (1 - white - black) + white), alpha];
    }
    return [...fromHsl(hue, first, second), alpha];
  };

  // The `font` shorthand, reduced to what shaping needs.
  //
  // Relative sizes resolve against 16px rather than against the canvas
  // element's own computed font, which is what the specification says to use.
  // Reading that would be a forced style resolution on every assignment, and
  // the difference is only visible to a canvas configured in `em` — see
  // COMPATIBILITY.md.
  const FONT_STYLES = { normal: 0, italic: 1, oblique: 2 };
  const FONT_WEIGHTS = { normal: 400, bold: 700, lighter: 100, bolder: 700 };
  const FONT_STRETCHES = {
    "ultra-condensed": 50, "extra-condensed": 62.5, condensed: 75, "semi-condensed": 87.5,
    normal: 100, "semi-expanded": 112.5, expanded: 125, "extra-expanded": 150,
    "ultra-expanded": 200,
  };
  const ABSOLUTE_SIZES = {
    "xx-small": 9, "x-small": 10, small: 13, medium: 16, large: 18, "x-large": 24,
    "xx-large": 32, "xxx-large": 48, smaller: 13, larger: 18,
  };
  const FONT_UNITS = { px: 1, pt: 4 / 3, pc: 16, in: 96, cm: 96 / 2.54, mm: 96 / 25.4, q: 96 / 101.6,
    em: 16, rem: 16, ex: 8, ch: 8, "%": 0.16 };
  const fontSizePixels = token => {
    if (ABSOLUTE_SIZES[token] !== undefined) return ABSOLUTE_SIZES[token];
    const match = /^([+-]?(?:\d+\.?\d*|\.\d+))(px|pt|pc|in|cm|mm|q|em|rem|ex|ch|%)$/.exec(token);
    if (!match) return null;
    const size = Number.parseFloat(match[1]) * FONT_UNITS[match[2]];
    return size >= 0 && Number.isFinite(size) ? size : null;
  };

  // Splits a shorthand into tokens, keeping a quoted family name whole.
  const fontTokens = text => text.match(/"[^"]*"|'[^']*'|[^\s,]+|,/g) ?? [];

  const parseCanvasFont = value => {
    // Tokenized with its case intact: keywords are matched case-insensitively
    // below, but the family list is handed back to `ctx.font` as it was
    // written, and lower-casing `'Helvetica Neue'` on the way through would be
    // a value the application did not set.
    const tokens = fontTokens(String(value).trim());
    const font = { style: 0, weight: 400, stretch: 100, size: 10, families: "sans-serif" };
    let index = 0;
    // Everything before the size is a keyword, in any order, and `normal` may
    // stand for any of the three properties that accept it.
    for (; index < tokens.length; index++) {
      const token = tokens[index].toLowerCase();
      if (token === "normal") continue;
      if (FONT_STYLES[token] !== undefined) { font.style = FONT_STYLES[token]; continue; }
      if (FONT_WEIGHTS[token] !== undefined) { font.weight = FONT_WEIGHTS[token]; continue; }
      if (FONT_STRETCHES[token] !== undefined) { font.stretch = FONT_STRETCHES[token]; continue; }
      if (/^[1-9]\d{0,2}$/.test(token) && Number(token) <= 1000) {
        font.weight = Number(token);
        continue;
      }
      // `small-caps` and the rest of `font-variant` are accepted and dropped:
      // the shorthand is still valid with one, and nothing below can apply it.
      if (token === "small-caps") continue;
      break;
    }
    if (index >= tokens.length) return null;
    // The size, and a line height this context has no use for.
    const [sizeToken] = tokens[index].toLowerCase().split("/");
    const size = fontSizePixels(sizeToken);
    if (size === null) return null;
    index++;
    const families = tokens.slice(index).join(" ").replace(/\s*,\s*/g, ", ").trim();
    if (!families) return null;
    font.size = size;
    // Re-quoted rather than passed through, because the renderer parses this as
    // a CSS family list and an unquoted name with a space in it is not one.
    font.families = families;
    return font;
  };

  // What `ctx.font` reads back as: the shorthand with every default left out,
  // which is how a browser serializes it.
  const serializeCanvasFont = font => {
    const parts = [];
    if (font.style === 1) parts.push("italic");
    if (font.style === 2) parts.push("oblique");
    if (font.weight !== 400) parts.push(String(font.weight));
    for (const [name, value] of Object.entries(FONT_STRETCHES))
      if (value === font.stretch && value !== 100) parts.push(name);
    parts.push(`${font.size}px`);
    parts.push(font.families);
    return parts.join(" ");
  };

  // 2D transforms as the six numbers everything here passes around, in the
  // order `DOMMatrix`, `setTransform` and the renderer all write them.
  const IDENTITY_MATRIX = [1, 0, 0, 1, 0, 0];
  const multiplyMatrix = (left, right) => [
    left[0] * right[0] + left[2] * right[1],
    left[1] * right[0] + left[3] * right[1],
    left[0] * right[2] + left[2] * right[3],
    left[1] * right[2] + left[3] * right[3],
    left[0] * right[4] + left[2] * right[5] + left[4],
    left[1] * right[4] + left[3] * right[5] + left[5],
  ];
  const invertMatrix = matrix => {
    const determinant = matrix[0] * matrix[3] - matrix[1] * matrix[2];
    if (!determinant || !Number.isFinite(determinant)) return null;
    return [
      matrix[3] / determinant, -matrix[1] / determinant,
      -matrix[2] / determinant, matrix[0] / determinant,
      (matrix[2] * matrix[5] - matrix[3] * matrix[4]) / determinant,
      (matrix[1] * matrix[4] - matrix[0] * matrix[5]) / determinant,
    ];
  };
  const finiteNumbers = values => values.every(value => Number.isFinite(value));

  // The 2D half of `DOMMatrix`, which is the half a 2D context has. `is2D` is
  // therefore always true and the 3D members are absent rather than reporting
  // an identity they could never leave — see the compatibility policy.
  class DOMMatrix {
    constructor(init) {
      let values = IDENTITY_MATRIX;
      if (Array.isArray(init) && init.length >= 6) values = init.slice(0, 6).map(Number);
      else if (init && typeof init === "object" && "a" in init)
        values = [init.a, init.b, init.c, init.d, init.e, init.f].map(Number);
      [this.a, this.b, this.c, this.d, this.e, this.f] = values;
    }
    get m11() { return this.a; }
    get m12() { return this.b; }
    get m21() { return this.c; }
    get m22() { return this.d; }
    get m41() { return this.e; }
    get m42() { return this.f; }
    get is2D() { return true; }
    get isIdentity() {
      return this.a === 1 && this.b === 0 && this.c === 0 && this.d === 1
        && this.e === 0 && this.f === 0;
    }
    multiply(other) { return new DOMMatrix(multiplyMatrix(matrixOf(this), matrixOf(other))); }
    translate(x = 0, y = 0) {
      return new DOMMatrix(multiplyMatrix(matrixOf(this), [1, 0, 0, 1, Number(x), Number(y)]));
    }
    scale(x = 1, y = x) {
      return new DOMMatrix(multiplyMatrix(matrixOf(this), [Number(x), 0, 0, Number(y), 0, 0]));
    }
    rotate(degrees = 0) {
      const radians = (Number(degrees) * Math.PI) / 180;
      return new DOMMatrix(multiplyMatrix(matrixOf(this),
        [Math.cos(radians), Math.sin(radians), -Math.sin(radians), Math.cos(radians), 0, 0]));
    }
    inverse() { return new DOMMatrix(invertMatrix(matrixOf(this)) ?? IDENTITY_MATRIX); }
    toJSON() {
      const { a, b, c, d, e, f } = this;
      return { a, b, c, d, e, f, is2D: true, isIdentity: this.isIdentity };
    }
    toString() {
      return `matrix(${this.a}, ${this.b}, ${this.c}, ${this.d}, ${this.e}, ${this.f})`;
    }
  }
  // Any of the three shapes an API here accepts a transform in: a matrix
  // object, a `DOMMatrixInit` dictionary, or the six numbers themselves.
  const matrixOf = value => {
    if (Array.isArray(value)) return value.slice(0, 6).map(Number);
    if (value && typeof value === "object")
      return [value.a ?? 1, value.b ?? 0, value.c ?? 0, value.d ?? 1, value.e ?? 0, value.f ?? 0]
        .map(Number);
    return IDENTITY_MATRIX;
  };

  // Path token kinds, matched by `blitsen-blitz`'s canvas wire reader.
  const PATH_MOVE = 0, PATH_LINE = 1, PATH_QUAD = 2, PATH_CUBIC = 3, PATH_CLOSE = 4;

  // A path as the token stream the renderer reads, built as it is described.
  //
  // The stream is the storage: there is no second representation to keep in
  // step, and `Path2D` and a context's current path are the same object under
  // two names. Arcs are flattened to cubics here because the renderer's path
  // has no arc segment and a browser's does not either — an SVG arc and a
  // canvas `arc` are both cubics by the time anything rasterises them.
  class CanvasPath {
    constructor() {
      this.tokens = [];
      this.startX = 0;
      this.startY = 0;
      this.x = 0;
      this.y = 0;
      this.open = false;
    }
    _ensureStart(x, y) {
      if (!this.open) this.moveTo(x, y);
    }
    moveTo(x, y) {
      x = Number(x); y = Number(y);
      if (!finiteNumbers([x, y])) return;
      this.tokens.push(PATH_MOVE, x, y);
      this.startX = this.x = x;
      this.startY = this.y = y;
      this.open = true;
    }
    lineTo(x, y) {
      x = Number(x); y = Number(y);
      if (!finiteNumbers([x, y])) return;
      this._ensureStart(x, y);
      this.tokens.push(PATH_LINE, x, y);
      this.x = x; this.y = y;
    }
    quadraticCurveTo(cx, cy, x, y) {
      const values = [cx, cy, x, y].map(Number);
      if (!finiteNumbers(values)) return;
      this._ensureStart(values[0], values[1]);
      this.tokens.push(PATH_QUAD, ...values);
      this.x = values[2]; this.y = values[3];
    }
    bezierCurveTo(c1x, c1y, c2x, c2y, x, y) {
      const values = [c1x, c1y, c2x, c2y, x, y].map(Number);
      if (!finiteNumbers(values)) return;
      this._ensureStart(values[0], values[1]);
      this.tokens.push(PATH_CUBIC, ...values);
      this.x = values[4]; this.y = values[5];
    }
    closePath() {
      if (!this.open) return;
      this.tokens.push(PATH_CLOSE);
      this.x = this.startX;
      this.y = this.startY;
    }
    rect(x, y, width, height) {
      const values = [x, y, width, height].map(Number);
      if (!finiteNumbers(values)) return;
      const [left, top, w, h] = values;
      this.moveTo(left, top);
      this.lineTo(left + w, top);
      this.lineTo(left + w, top + h);
      this.lineTo(left, top + h);
      this.closePath();
      // A rectangle leaves a fresh subpath at its own origin, which is what
      // makes `rect(); lineTo()` draw from the corner rather than continue.
      this.moveTo(left, top);
    }
    roundRect(x, y, width, height, radii = 0) {
      const values = [x, y, width, height].map(Number);
      if (!finiteNumbers(values)) return;
      const [left, top, w, h] = values;
      const list = (Array.isArray(radii) ? radii : [radii]).map(value =>
        typeof value === "object" && value !== null ? Number(value.x ?? 0) : Number(value));
      if (!finiteNumbers(list) || list.some(value => value < 0))
        throw new RangeError("roundRect radii must be non-negative numbers");
      // One, two, three or four radii, expanded the way the CSS corner
      // shorthand expands: the same rule `border-radius` follows.
      const [topLeft, topRight, bottomRight, bottomLeft] = {
        1: [list[0], list[0], list[0], list[0]],
        2: [list[0], list[1], list[0], list[1]],
        3: [list[0], list[1], list[2], list[1]],
        4: list,
      }[Math.min(list.length, 4)] ?? [0, 0, 0, 0];
      const limit = Math.min(Math.abs(w), Math.abs(h)) / 2;
      const corners = [topLeft, topRight, bottomRight, bottomLeft].map(value =>
        Math.min(value, limit));
      const [x0, y0, x1, y1] = [Math.min(left, left + w), Math.min(top, top + h),
        Math.max(left, left + w), Math.max(top, top + h)];
      this.moveTo(x0 + corners[0], y0);
      this.lineTo(x1 - corners[1], y0);
      this.arcTo(x1, y0, x1, y0 + corners[1], corners[1]);
      this.lineTo(x1, y1 - corners[2]);
      this.arcTo(x1, y1, x1 - corners[2], y1, corners[2]);
      this.lineTo(x0 + corners[3], y1);
      this.arcTo(x0, y1, x0, y1 - corners[3], corners[3]);
      this.lineTo(x0, y0 + corners[0]);
      this.arcTo(x0, y0, x0 + corners[0], y0, corners[0]);
      this.closePath();
      this.moveTo(x0 + corners[0], y0);
    }
    // An elliptical arc, flattened into cubics no wider than a quarter turn —
    // the span over which a cubic approximates a circular arc to within a
    // fraction of a device pixel at any size a canvas is drawn at.
    ellipse(cx, cy, radiusX, radiusY, rotation, start, end, counterclockwise = false) {
      const values = [cx, cy, radiusX, radiusY, rotation, start, end].map(Number);
      if (!finiteNumbers(values)) return;
      const [centerX, centerY, rx, ry, angle, startAngle, endAngle] = values;
      if (rx < 0 || ry < 0) throw new RangeError("arc radii must be non-negative");
      let sweep = endAngle - startAngle;
      const full = Math.PI * 2;
      if (counterclockwise) sweep = sweep > 0 ? ((sweep % full) - full) || -full : Math.max(sweep, -full);
      else sweep = sweep < 0 ? ((sweep % full) + full) || full : Math.min(sweep, full);
      const [cos, sin] = [Math.cos(angle), Math.sin(angle)];
      const at = theta => {
        const [ex, ey] = [rx * Math.cos(theta), ry * Math.sin(theta)];
        return [centerX + ex * cos - ey * sin, centerY + ex * sin + ey * cos];
      };
      const derivative = theta => {
        const [ex, ey] = [-rx * Math.sin(theta), ry * Math.cos(theta)];
        return [ex * cos - ey * sin, ex * sin + ey * cos];
      };
      const [firstX, firstY] = at(startAngle);
      if (this.open) this.lineTo(firstX, firstY);
      else this.moveTo(firstX, firstY);
      const segments = Math.max(1, Math.ceil(Math.abs(sweep) / (Math.PI / 2)));
      const step = sweep / segments;
      // The control-point distance that makes a cubic match a circular arc of
      // this span at its endpoints and its midpoint.
      const alpha = (4 / 3) * Math.tan(step / 4);
      for (let index = 0; index < segments; index++) {
        const from = startAngle + step * index;
        const to = from + step;
        const [px, py] = at(from);
        const [qx, qy] = at(to);
        const [dpx, dpy] = derivative(from);
        const [dqx, dqy] = derivative(to);
        this.bezierCurveTo(px + alpha * dpx, py + alpha * dpy,
          qx - alpha * dqx, qy - alpha * dqy, qx, qy);
      }
    }
    arc(cx, cy, radius, start, end, counterclockwise = false) {
      this.ellipse(cx, cy, radius, radius, 0, start, end, counterclockwise);
    }
    // The corner-rounding form: the arc of the given radius tangent to both
    // legs of the corner, plus the line that reaches it.
    arcTo(x1, y1, x2, y2, radius) {
      const values = [x1, y1, x2, y2, radius].map(Number);
      if (!finiteNumbers(values)) return;
      const [ax, ay, bx, by, r] = values;
      if (r < 0) throw new RangeError("arcTo radius must be non-negative");
      this._ensureStart(ax, ay);
      const [ux, uy] = [this.x - ax, this.y - ay];
      const [vx, vy] = [bx - ax, by - ay];
      const cross = ux * vy - uy * vx;
      // Collinear legs, a zero radius or a degenerate corner: the arc has no
      // room to exist and the specification says to draw the line instead.
      if (cross === 0 || r === 0 || (ux === 0 && uy === 0) || (vx === 0 && vy === 0)) {
        this.lineTo(ax, ay);
        return;
      }
      const [ul, vl] = [Math.hypot(ux, uy), Math.hypot(vx, vy)];
      // Clamped, because a cosine that rounds to 1.0000000000000002 makes
      // `acos` a NaN and the whole corner disappears.
      const cosine = Math.min(1, Math.max(-1, (ux * vx + uy * vy) / (ul * vl)));
      const half = Math.acos(cosine) / 2;
      const along = r / Math.tan(half);
      const [t1x, t1y] = [ax + (ux / ul) * along, ay + (uy / ul) * along];
      const [t2x, t2y] = [ax + (vx / vl) * along, ay + (vy / vl) * along];
      const bisector = Math.hypot(ux / ul + vx / vl, uy / ul + vy / vl);
      const distance = r / Math.sin(half);
      const centerX = ax + ((ux / ul + vx / vl) / bisector) * distance;
      const centerY = ay + ((uy / ul + vy / vl) / bisector) * distance;
      this.lineTo(t1x, t1y);
      this.ellipse(centerX, centerY, r, r, 0,
        Math.atan2(t1y - centerY, t1x - centerX),
        Math.atan2(t2y - centerY, t2x - centerX), cross > 0);
    }
    // Appends another path, optionally through a transform. Tokens rather than
    // calls, so a transformed copy costs one pass and no re-flattening.
    addPath(path, transform) {
      if (!(path instanceof Path2D)) throw new TypeError("addPath needs a Path2D");
      const matrix = transform === undefined ? null : matrixOf(transform);
      const tokens = path.tokens;
      for (let index = 0; index < tokens.length;) {
        const kind = tokens[index++];
        const points = { [PATH_MOVE]: 1, [PATH_LINE]: 1, [PATH_QUAD]: 2, [PATH_CUBIC]: 3,
          [PATH_CLOSE]: 0 }[kind];
        this.tokens.push(kind);
        for (let point = 0; point < points; point++) {
          const [x, y] = [tokens[index++], tokens[index++]];
          this.tokens.push(matrix ? matrix[0] * x + matrix[2] * y + matrix[4] : x,
            matrix ? matrix[1] * x + matrix[3] * y + matrix[5] : y);
          this.x = x; this.y = y;
        }
        if (kind !== PATH_CLOSE) this.open = true;
      }
    }
  }

  // SVG path data, for `new Path2D("M0 0 L10 10Z")`.
  //
  // The whole grammar, because a path string is generated by a tool far more
  // often than it is written by hand and a tool emits every command. Arcs go
  // through the endpoint-to-centre conversion the SVG specification gives, and
  // then through the same flattening a canvas `ellipse` uses.
  const SVG_ARGUMENTS = { m: 2, l: 2, h: 1, v: 1, c: 6, s: 4, q: 4, t: 2, a: 7, z: 0 };
  const applySvgPathData = (path, data) => {
    const tokens = String(data).match(/[astvzqmhlc]|[+-]?(?:\d*\.\d+|\d+\.?)(?:[eE][+-]?\d+)?/gi);
    if (!tokens) return;
    let command = null;
    let index = 0;
    let [lastControlX, lastControlY] = [null, null];
    while (index < tokens.length) {
      if (/[a-z]/i.test(tokens[index])) command = tokens[index++];
      if (!command) return;
      const lower = command.toLowerCase();
      const arity = SVG_ARGUMENTS[lower];
      if (arity === undefined) return;
      const relative = command !== lower;
      const args = tokens.slice(index, index + arity).map(Number);
      if (args.length < arity) return;
      index += arity;
      const [ox, oy] = relative ? [path.x, path.y] : [0, 0];
      const [beforeX, beforeY] = [path.x, path.y];
      let control = null;
      switch (lower) {
        case "m": path.moveTo(ox + args[0], oy + args[1]); break;
        case "l": path.lineTo(ox + args[0], oy + args[1]); break;
        case "h": path.lineTo(ox + args[0], path.y); break;
        case "v": path.lineTo(path.x, oy + args[0]); break;
        case "c":
          control = [ox + args[2], oy + args[3]];
          path.bezierCurveTo(ox + args[0], oy + args[1], control[0], control[1],
            ox + args[4], oy + args[5]);
          break;
        case "s": {
          const [rx, ry] = lastControlX === null ? [beforeX, beforeY]
            : [2 * beforeX - lastControlX, 2 * beforeY - lastControlY];
          control = [ox + args[0], oy + args[1]];
          path.bezierCurveTo(rx, ry, control[0], control[1], ox + args[2], oy + args[3]);
          break;
        }
        case "q":
          control = [ox + args[0], oy + args[1]];
          path.quadraticCurveTo(control[0], control[1], ox + args[2], oy + args[3]);
          break;
        case "t": {
          const [rx, ry] = lastControlX === null ? [beforeX, beforeY]
            : [2 * beforeX - lastControlX, 2 * beforeY - lastControlY];
          control = [rx, ry];
          path.quadraticCurveTo(rx, ry, ox + args[0], oy + args[1]);
          break;
        }
        case "a": svgArc(path, args, ox, oy); break;
        case "z": path.closePath(); break;
      }
      [lastControlX, lastControlY] = control ?? [null, null];
      // A repeated `m` continues as `l`, which is what makes a polygon written
      // as one move and a list of pairs close correctly.
      if (lower === "m") command = relative ? "l" : "L";
    }
  };
  const svgArc = (path, [rx, ry, rotation, largeArc, sweep, x, y], ox, oy) => {
    const [endX, endY] = [ox + x, oy + y];
    const [startX, startY] = [path.x, path.y];
    if (!rx || !ry) { path.lineTo(endX, endY); return; }
    const angle = (rotation * Math.PI) / 180;
    const [cos, sin] = [Math.cos(angle), Math.sin(angle)];
    const [dx, dy] = [(startX - endX) / 2, (startY - endY) / 2];
    const [x1, y1] = [cos * dx + sin * dy, -sin * dx + cos * dy];
    let [a, b] = [Math.abs(rx), Math.abs(ry)];
    const oversize = (x1 * x1) / (a * a) + (y1 * y1) / (b * b);
    if (oversize > 1) { a *= Math.sqrt(oversize); b *= Math.sqrt(oversize); }
    const denominator = a * a * y1 * y1 + b * b * x1 * x1;
    const scale = Math.sqrt(Math.max(0, (a * a * b * b - denominator) / denominator))
      * (largeArc !== 0 === (sweep !== 0) ? -1 : 1);
    const [cx1, cy1] = [(scale * a * y1) / b, (-scale * b * x1) / a];
    const centerX = cos * cx1 - sin * cy1 + (startX + endX) / 2;
    const centerY = sin * cx1 + cos * cy1 + (startY + endY) / 2;
    const start = Math.atan2((y1 - cy1) / b, (x1 - cx1) / a);
    let end = Math.atan2((-y1 - cy1) / b, (-x1 - cx1) / a);
    if (sweep === 0 && end > start) end -= Math.PI * 2;
    if (sweep !== 0 && end < start) end += Math.PI * 2;
    path.ellipse(centerX, centerY, a, b, angle, start, end, sweep === 0);
  };

  class Path2D extends CanvasPath {
    constructor(source) {
      super();
      if (source instanceof Path2D) this.addPath(source);
      else if (source !== undefined) applySvgPathData(this, source);
    }
  }

  // A gradient's stops, held until a draw uses it: the coordinates are in the
  // user space of the *draw* rather than of the creation, so nothing here is
  // resolved until then.
  class CanvasGradient {
    constructor(kind, geometry) {
      this._kind = kind;
      this._geometry = geometry;
      this._stops = [];
    }
    addColorStop(offset, color) {
      offset = Number(offset);
      if (!Number.isFinite(offset) || offset < 0 || offset > 1)
        throw new DOMException("Gradient stop offsets must be between 0 and 1", "IndexSizeError");
      const rgba = parseCanvasColor(color);
      if (!rgba) throw new DOMException("Unparseable gradient stop colour", "SyntaxError");
      this._stops.push([offset, rgba]);
      this._stops.sort((left, right) => left[0] - right[0]);
    }
  }

  const PATTERN_EXTENDS = { repeat: [1, 1], "repeat-x": [1, 0], "repeat-y": [0, 1],
    "no-repeat": [0, 0] };

  class CanvasPattern {
    constructor(source, repetition) {
      this._source = source;
      this._extends = repetition;
      this._transform = IDENTITY_MATRIX;
    }
    setTransform(transform) { this._transform = matrixOf(transform); }
  }

  // Pixels an application owns, in straight-alpha RGBA8 rows. The buffer is the
  // object: `data` is not a copy, so writing through it is what `putImageData`
  // then puts.
  class ImageData {
    constructor(first, second, third) {
      let pixels = null;
      let [width, height] = [0, 0];
      if (ArrayBuffer.isView(first)) {
        pixels = first;
        width = Number(second);
        height = third === undefined ? pixels.length / 4 / width : Number(third);
        if (!(pixels instanceof Uint8ClampedArray))
          throw new TypeError("ImageData pixels must be a Uint8ClampedArray");
        if (pixels.length !== width * height * 4)
          throw new DOMException("ImageData size does not match its pixel buffer",
            "InvalidStateError");
      } else {
        width = Number(first);
        height = Number(second);
      }
      if (!(width > 0) || !(height > 0) || !Number.isInteger(width) || !Number.isInteger(height))
        throw new DOMException("ImageData dimensions must be positive integers", "IndexSizeError");
      defineMembers(this, {
        width, height,
        data: pixels ?? new Uint8ClampedArray(width * height * 4),
        colorSpace: "srgb",
      });
    }
  }

  // A measurement, frozen: it is a reading taken at a moment, not a handle onto
  // the text that was measured.
  class TextMetrics {
    constructor(values) {
      const [width, left, right, ascent, descent, fontAscent, fontDescent] = values;
      defineMembers(this, {
        width,
        actualBoundingBoxLeft: left,
        actualBoundingBoxRight: right,
        actualBoundingBoxAscent: ascent,
        actualBoundingBoxDescent: descent,
        fontBoundingBoxAscent: fontAscent,
        fontBoundingBoxDescent: fontDescent,
        // The em box, which for every face this runtime shapes with is the
        // typographic ascent and descent it already reported.
        emHeightAscent: fontAscent,
        emHeightDescent: fontDescent,
        alphabeticBaseline: 0,
        hangingBaseline: fontAscent * 0.8,
        ideographicBaseline: -fontDescent,
      });
      Object.freeze(this);
    }
  }

// React Flow reads the viewport zoom through DOMMatrixReadOnly. Blitsen does
// not expose the geometry interfaces yet, so provide the small read-only slice
// the library uses while leaving browsers on their native implementation.
if (typeof globalThis.DOMMatrixReadOnly === "undefined") {
  const numbers = value =>
    (String(value).match(/[-+]?(?:\d*\.)?\d+(?:e[-+]?\d+)?/gi) ?? []).map(Number);

  class DOMMatrixReadOnly {
    constructor(transform = "none") {
      let a = 1;
      let b = 0;
      let c = 0;
      let d = 1;
      let e = 0;
      let f = 0;
      const source = String(transform).trim();

      if (source.startsWith("matrix3d(")) {
        const values = numbers(source.slice(source.indexOf("(") + 1));
        [a, b, c, d, e, f] = [
          values[0],
          values[1],
          values[4],
          values[5],
          values[12],
          values[13],
        ];
      } else if (source.startsWith("matrix(")) {
        [a, b, c, d, e, f] = numbers(source.slice(source.indexOf("(") + 1));
      } else if (source !== "none" && source !== "") {
        for (const match of source.matchAll(/(translate|scale)(3d|X|Y)?\(([^)]*)\)/gi)) {
          const [first = 0, second] = numbers(match[3]);
          const operation = match[1].toLowerCase();
          const axis = match[2]?.toLowerCase();
          if (operation === "translate") {
            if (axis !== "y") e += first;
            if (axis !== "x") f += axis === "y" ? first : (second ?? 0);
          } else {
            if (axis !== "y") a *= first;
            if (axis !== "x") d *= axis === "y" ? first : (second ?? first);
          }
        }
      }

      Object.assign(this, {
        a, b, c, d, e, f,
        m11: a, m12: b, m13: 0, m14: 0,
        m21: c, m22: d, m23: 0, m24: 0,
        m31: 0, m32: 0, m33: 1, m34: 0,
        m41: e, m42: f, m43: 0, m44: 1,
      });
    }

    get is2D() { return true; }
    get isIdentity() {
      return this.a === 1 && this.b === 0 && this.c === 0 &&
        this.d === 1 && this.e === 0 && this.f === 0;
    }

    toString() {
      return `matrix(${this.a}, ${this.b}, ${this.c}, ${this.d}, ${this.e}, ${this.f})`;
    }
  }

  globalThis.DOMMatrixReadOnly = DOMMatrixReadOnly;
  globalThis.DOMMatrix = DOMMatrixReadOnly;
  globalThis.WebKitCSSMatrix = DOMMatrixReadOnly;
}

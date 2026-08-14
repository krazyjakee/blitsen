// Blitsen lays out and paints canvas elements, but its DOM bridge does not yet
// expose getContext(). Keep Monaco on the native canvas element and provide the
// small 2D surface it needs until that bridge API lands.
const originalCreateElement = document.createElement.bind(document);
const canvasContexts = new WeakMap();

const measureText = (text, font) => {
  const probe = originalCreateElement("span");
  probe.style.cssText = [
    "position: absolute",
    "left: -10000px",
    "top: -10000px",
    "white-space: pre",
    `font: ${font}`,
  ].join(";");
  probe.textContent = String(text);
  document.body.appendChild(probe);

  const range = document.createRange();
  range.selectNodeContents(probe);
  const width = range.getBoundingClientRect().width;
  probe.remove();
  return { width };
};

const createCanvasContext = () => ({
  backingStorePixelRatio: 1,
  webkitBackingStorePixelRatio: 1,
  mozBackingStorePixelRatio: 1,
  msBackingStorePixelRatio: 1,
  oBackingStorePixelRatio: 1,
  font: "14px monospace",
  fillStyle: "transparent",
  strokeStyle: "transparent",
  clearRect() {},
  fillRect() {},
  strokeRect() {},
  beginPath() {},
  closePath() {},
  moveTo() {},
  lineTo() {},
  arc() {},
  fill() {},
  stroke() {},
  save() {},
  restore() {},
  translate() {},
  scale() {},
  rotate() {},
  setTransform() {},
  resetTransform() {},
  drawImage() {},
  putImageData() {},
  createLinearGradient() {
    return { addColorStop() {} };
  },
  createRadialGradient() {
    return { addColorStop() {} };
  },
  measureText(text) {
    return measureText(text, this.font);
  },
});

document.createElement = function createElement(tagName, options) {
  const element = originalCreateElement(tagName, options);
  if (String(tagName).toLowerCase() !== "canvas" || typeof element.getContext === "function") {
    return element;
  }

  element.getContext = (type) => {
    if (type !== "2d") {
      return null;
    }
    if (!canvasContexts.has(element)) {
      canvasContexts.set(element, createCanvasContext());
    }
    return canvasContexts.get(element);
  };
  return element;
};

async function mountEditor() {
  const [{ default: EditorWorker }, { default: TypeScriptWorker }] =
    await Promise.all([
    import("monaco-editor/esm/vs/editor/editor.worker?worker"),
    import("monaco-editor/esm/vs/language/typescript/ts.worker?worker"),
    ]);

  self.MonacoEnvironment = {
    getWorker(_moduleId, label) {
      if (label === "javascript" || label === "typescript") {
        return new TypeScriptWorker();
      }

      return new EditorWorker();
    },
  };

  const monaco = await import("monaco-editor/esm/vs/editor/editor.api");

  await Promise.all([
    import("monaco-editor/esm/vs/basic-languages/javascript/javascript.contribution"),
    import("monaco-editor/esm/vs/language/typescript/monaco.contribution"),
  ]);

  monaco.editor.create(document.getElementById("editor"), {
    value: [
      "function greet(name) {",
      "  return `Hello, ${name}!`;",
      "}",
      "",
      "console.log(greet(\"world\"));",
    ].join("\n"),
    language: "javascript",
    theme: "vs-dark",
    automaticLayout: true,
    minimap: { enabled: false },
    overviewRulerLanes: 0,
    renderValidationDecorations: "off",
    experimentalGpuAcceleration: "off",
  });
}

mountEditor().catch((error) => console.error("Unable to mount Monaco:", error));

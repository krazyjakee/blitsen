const originalCreateElement = document.createElement.bind(document);
const canvasContexts = new WeakMap();

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
    const fontSize = Number.parseFloat(this.font.match(/(\d+(?:\.\d+)?)px/)?.[1]) || 14;
    return { width: String(text).length * fontSize * 0.6 };
  },
});

document.createElement = function createElement(tagName, options) {
  const isCanvas = String(tagName).toLowerCase() === "canvas";
  let element;

  try {
    element = originalCreateElement(tagName, options);
  } catch (error) {
    if (!isCanvas) {
      throw error;
    }

    element = originalCreateElement("div");
  }

  let hasNative2dContext = false;

  if (isCanvas) {
    try {
      hasNative2dContext = Boolean(element.getContext?.("2d"));
    } catch {
      // Blitsen may expose a placeholder method that throws for unsupported APIs.
    }
  }

  if (isCanvas && !hasNative2dContext) {
    const getContext = (type) => {
      if (type !== "2d") {
        return null;
      }

      if (!canvasContexts.has(element)) {
        canvasContexts.set(element, createCanvasContext());
      }

      return canvasContexts.get(element);
    };

    try {
      element.getContext = getContext;
    } catch {
      Object.defineProperty(element, "getContext", { value: getContext });
    }
  }

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

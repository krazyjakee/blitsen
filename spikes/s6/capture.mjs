const CDP_PORT = 49222;
const WIDTH = 1440;
const HEIGHT = 800;
const SCALE = 2;

const required = (name) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
};

const chromium = process.env.CHROMIUM ?? "chromium";
const outputDir = required("S6_OUTPUT_DIR");
const profileDir = required("S6_CHROMIUM_PROFILE");

const apps = [
  {
    name: "react",
    url: "http://127.0.0.1:48101/",
    snapshot: `${required("S6_REACT_DIST")}/snapshot.html`,
  },
  {
    name: "vue",
    url: "http://127.0.0.1:48102/",
    snapshot: `${required("S6_VUE_DIST")}/snapshot.html`,
  },
  {
    name: "svelte",
    url: "http://127.0.0.1:48104/wordle/",
    snapshot: `${required("S6_SVELTE_DIST")}/snapshot.html`,
    beforeReload:
      'localStorage.setItem("settings", JSON.stringify({hard:[false,false,false],dark:false,colorblind:false,tutorial:0}))',
  },
];

const chrome = Bun.spawn(
  [
    chromium,
    "--headless",
    "--disable-gpu",
    "--no-sandbox",
    "--hide-scrollbars",
    `--remote-debugging-port=${CDP_PORT}`,
    "--remote-debugging-address=127.0.0.1",
    `--user-data-dir=${profileDir}`,
    `--window-size=${WIDTH},${HEIGHT}`,
    "about:blank",
  ],
  { stdout: "ignore", stderr: "inherit" },
);

async function waitForDevtools() {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`);
      if (response.ok) return;
    } catch {
      // Chromium has not opened its debugging socket yet.
    }
    await Bun.sleep(100);
  }
  throw new Error("Chromium DevTools endpoint did not become ready");
}

async function openTarget(url) {
  const response = await fetch(
    `http://127.0.0.1:${CDP_PORT}/json/new?${encodeURIComponent(url)}`,
    { method: "PUT" },
  );
  if (!response.ok) throw new Error(`Could not open ${url}: ${response.status}`);
  return response.json();
}

async function connect(webSocketDebuggerUrl) {
  const socket = new WebSocket(webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });

  let nextId = 0;
  const pending = new Map();
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (!message.id || !pending.has(message.id)) return;
    const promise = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) promise.reject(new Error(JSON.stringify(message.error)));
    else promise.resolve(message.result);
  };

  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++nextId;
      pending.set(id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });

  return { socket, send };
}

async function capture(app) {
  const target = await openTarget(app.url);
  const { socket, send } = await connect(target.webSocketDebuggerUrl);
  await send("Page.enable");
  await send("Runtime.enable");
  await send("Emulation.setDeviceMetricsOverride", {
    width: WIDTH,
    height: HEIGHT,
    deviceScaleFactor: SCALE,
    mobile: false,
  });
  await send("Page.navigate", { url: app.url });
  await Bun.sleep(500);

  if (app.beforeReload) {
    await send("Runtime.evaluate", { expression: app.beforeReload });
    await send("Page.reload", { ignoreCache: true });
  }

  // The fixtures are local; this allows framework rendering and font/layout settling.
  await Bun.sleep(2500);
  const screenshot = await send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    clip: { x: 0, y: 0, width: WIDTH, height: HEIGHT, scale: 1 },
  });
  await Bun.write(
    `${outputDir}/chromium/${app.name}.png`,
    Buffer.from(screenshot.data, "base64"),
  );

  const dom = await send("Runtime.evaluate", {
    expression: "document.documentElement.outerHTML",
    returnByValue: true,
  });
  await Bun.write(app.snapshot, `<!DOCTYPE html>\n${dom.result.value}\n`);
  socket.close();
}

try {
  await waitForDevtools();
  for (const app of apps) await capture(app);
} finally {
  chrome.kill();
  await chrome.exited;
}

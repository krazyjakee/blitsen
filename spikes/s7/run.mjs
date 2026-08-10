import { createServer } from "vite";

const fixtureRoot = new URL("./fixture/", import.meta.url).pathname;
const requests = new Map();

const countRequests = {
  name: "s7-count-and-redirect",
  configureServer(server) {
    server.middlewares.use((request, response, next) => {
      const url = new URL(request.url, "http://fixture.invalid");
      requests.set(url.pathname, (requests.get(url.pathname) ?? 0) + 1);
      if (url.pathname === "/entry") {
        response.statusCode = 302;
        response.setHeader("Location", "/index.html");
        response.end();
        return;
      }
      if (url.pathname === "/src/redirected.js") {
        response.statusCode = 302;
        response.setHeader("Location", "/src/redirect-target.js");
        response.end();
        return;
      }
      next();
    });
  },
};

const server = await createServer({
  root: fixtureRoot,
  logLevel: "silent",
  plugins: [countRequests],
  server: { host: "127.0.0.1", port: 0, strictPort: false },
});
await server.listen();
const address = server.httpServer.address();
const origin = `http://127.0.0.1:${address.port}`;

const fetched = new Map();
const canonical = new Map();

async function fetchModule(requestedUrl) {
  if (!fetched.has(requestedUrl)) {
    fetched.set(
      requestedUrl,
      fetch(requestedUrl).then(async (response) => {
        const contents = await response.text();
        if (!response.ok) {
          throw new Error(`HTTP ${response.status} loading ${requestedUrl}: ${contents}`);
        }
        const result = {
          finalUrl: response.url,
          contentType: response.headers.get("content-type") ?? "",
          contents,
        };
        canonical.set(requestedUrl, result.finalUrl);
        canonical.set(result.finalUrl, result.finalUrl);
        if (!fetched.has(result.finalUrl)) {
          fetched.set(result.finalUrl, Promise.resolve(result));
        }
        return result;
      }),
    );
  }
  return fetched.get(requestedUrl);
}

function resolveHttpSpecifier(path, importer) {
  const base = importer.startsWith("http:http")
    ? importer.slice("http:".length)
    : importer.startsWith("/http://") || importer.startsWith("/https://")
      ? importer.slice(1)
      : importer;
  const requested = path.startsWith("//")
    ? `http:${path}`
    : /^https?:\/\//.test(path)
      ? path
      : new URL(path, base).href;
  return canonical.get(requested) ?? requested;
}

const transpiler = new Bun.Transpiler({ loader: "js" });
async function prefetchModuleGraph(entryUrl, seen = new Set()) {
  const loaded = await fetchModule(entryUrl);
  if (seen.has(loaded.finalUrl)) return;
  seen.add(loaded.finalUrl);
  const imports = transpiler.scanImports(loaded.contents);
  await Promise.all(
    imports.map(({ path }) => {
      if (!path.startsWith(".") && !path.startsWith("/") && !path.startsWith("http")) {
        throw new Error(`unresolved bare HTTP import ${path} from ${loaded.finalUrl}`);
      }
      return prefetchModuleGraph(new URL(path, loaded.finalUrl).href, seen);
    }),
  );
}

Bun.plugin({
  name: "s7-http-modules",
  setup(build) {
    build.onResolve({ filter: /^https?:\/\// }, ({ path, importer }) => {
      return { path: resolveHttpSpecifier(path, importer), namespace: "http" };
    });
    build.onResolve({ filter: /.*/ }, ({ path, importer }) => {
      if (path.startsWith("//") || importer.startsWith("http:") || importer.startsWith("/http")) {
        return { path: resolveHttpSpecifier(path, importer), namespace: "http" };
      }
      return undefined;
    });
    build.onLoad({ filter: /.*/, namespace: "http" }, async ({ path }) => {
      const url = path.startsWith("//")
        ? `http:${path}`
        : path.startsWith("http:http")
          ? path.slice("http:".length)
          : path;
      const loaded = await fetchModule(url);
      const loader = loaded.contentType.includes("json") ? "json" : "js";
      return { contents: loaded.contents, loader };
    });
  },
});

async function parseDocument(entryUrl) {
  const response = await fetch(entryUrl);
  if (!response.ok) throw new Error(`HTTP ${response.status} loading document`);
  const resources = [];
  const collect = (attribute, kind) => ({
    element(element) {
      const value = element.getAttribute(attribute);
      if (value) resources.push({ kind, url: new URL(value, response.url).href });
    },
  });
  const parsedHtml = await new HTMLRewriter()
    .on("link[href]", collect("href", "style"))
    .on("img[src]", collect("src", "image"))
    .on("script[type=module][src]", collect("src", "module"))
    .transform(response)
    .text();
  if (!parsedHtml.includes("<body>")) throw new Error("document parse failed");
  return { finalUrl: response.url, resources };
}

function awaitHmrConnected(url) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url, "vite-hmr");
    let connected = false;
    const timer = setTimeout(() => {
      socket.close();
      reject(new Error("Vite HMR handshake timed out"));
    }, 3000);
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.type === "connected") {
        connected = true;
        socket.close();
      }
    });
    socket.addEventListener("close", () => {
      clearTimeout(timer);
      if (connected) resolve("connected");
      else reject(new Error("Vite HMR socket closed before handshake"));
    });
    socket.addEventListener("error", reject);
  });
}

try {
  const document = await parseDocument(`${origin}/entry`);
  const stylesheet = document.resources.find(({ kind }) => kind === "style");
  const image = document.resources.find(({ kind }) => kind === "image");
  const entry = document.resources.find(
    ({ kind, url }) => kind === "module" && url.endsWith("/src/main.js"),
  );
  if (!stylesheet || !image || !entry) throw new Error("missing parsed resources");

  const [cssResponse, imageResponse] = await Promise.all([
    fetch(stylesheet.url),
    fetch(image.url),
  ]);
  if (!cssResponse.ok || !(await cssResponse.text()).includes("rgb(12, 34, 56)")) {
    throw new Error("stylesheet fetch failed");
  }
  if (!imageResponse.ok || imageResponse.headers.get("content-type") !== "image/svg+xml") {
    throw new Error("image fetch failed");
  }

  await prefetchModuleGraph(entry.url);
  const module = await import(entry.url);
  const result = await module.run();
  if (result.total !== 42 || result.lazy !== "dynamic-import-ok" || result.redirected !== 7) {
    throw new Error(`module graph returned ${JSON.stringify(result)}`);
  }
  if (!result.assetUrl.startsWith("data:image/svg+xml,")) {
    throw new Error(`unexpected Vite asset URL ${result.assetUrl}`);
  }
  if ((await import(entry.url)) !== module) throw new Error("module cache miss");

  const sourceMap = await fetch(`${origin}/src/main.js.map`);
  const sourceMapContentType = sourceMap.headers.get("content-type") ?? "";
  const hmr = await awaitHmrConnected(`ws://127.0.0.1:${address.port}`);
  const trace = {
    bun: Bun.version,
    documentUrl: document.finalUrl,
    resources: document.resources,
    moduleResult: result,
    redirectedModule: canonical.get(`${origin}/src/redirected.js`),
    requestCounts: Object.fromEntries([...requests].sort()),
    sourceMap: {
      externalMap: sourceMap.ok && !sourceMapContentType.includes("text/html"),
      moduleIdentityPreserved: result.moduleUrl === entry.url,
      observedModuleUrl: result.moduleUrl,
    },
    hmr,
    globals: {
      fetch: typeof fetch,
      WebSocket: typeof WebSocket,
      EventSource: typeof EventSource,
    },
  };
  console.log(JSON.stringify(trace, null, 2));
} finally {
  void server.ws.close();
  void server.close();
}
process.exit(0);

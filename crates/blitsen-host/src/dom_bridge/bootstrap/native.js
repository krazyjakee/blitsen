  // The `native:` modules. `packages/blitsen/src/native/module.mjs` proxies every
  // `blitsen/<module>` subpath onto the namespace installed below, and reports a
  // member it cannot find as absent rather than as an error — so a capability the
  // host could not install must not appear here at all, and a member is dropped
  // when its host function is missing rather than stubbed with a thrower.
  //
  // Only capability with no Node or web spelling belongs here: argv, the
  // executable path and exit stay `process.argv`, `process.execPath` and
  // `process.exit` (TECH.md §9).
  //
  // Which host functions exist is a per-platform decision, not a per-build
  // accident: `dom_bridge/native.rs` installs a family only where the platform
  // can answer it honestly, and on Android that is `os` and the monitor list and
  // nothing else (#147). So every member below is written against a `hosted`
  // flag rather than reaching for its host function directly — an uninstalled
  // name is not merely falsy, it is undeclared, and a member that closed over it
  // would be a function `if (app.dataDir)` accepts and the first call throws on.
  const nativeMembers = members => Object.freeze(Object.fromEntries(
    Object.entries(members).filter(([, member]) => member !== undefined)));
  const hosted = name => typeof globalThis[name] === "function";
  const nativePending = typeof __blitsenNativeAppPending === "function"
    ? __blitsenNativeAppPending : () => false;
  let secondInstanceHandler = null;
  // Second invocations land here and nowhere else in the turn, for the same
  // reason `fetch` completions do: an application must never be re-entered from
  // a listener thread part-way through a frame.
  const deliverSecondInstances = () => {
    if (!nativePending()) return;
    for (const invocation of JSON.parse(__blitsenNativeAppSecondInstances())) {
      Object.freeze(invocation.argv);
      try { secondInstanceHandler?.(Object.freeze(invocation)); }
      catch (error) { console.error("Uncaught exception in the second-instance handler", error); }
    }
  };
  const appDirectories = hosted("__blitsenNativeAppDirectory");
  const nativeApp = {
    dataDir: appDirectories ? name => __blitsenNativeAppDirectory("data", String(name)) : undefined,
    cacheDir: appDirectories ? name => __blitsenNativeAppDirectory("cache", String(name)) : undefined,
    configDir: appDirectories ? name => __blitsenNativeAppDirectory("config", String(name)) : undefined,
    relaunch: hosted("__blitsenNativeAppRelaunch")
      ? () => { __blitsenNativeAppRelaunch(); }
      : undefined,
    requestSingleInstanceLock: typeof __blitsenNativeAppSingleInstance === "function"
      ? (name, onSecondInstance = null) => {
          if (onSecondInstance !== null && typeof onSecondInstance !== "function")
            throw new TypeError("the second-instance handler must be a function");
          const primary = __blitsenNativeAppSingleInstance(String(name));
          if (primary) secondInstanceHandler = onSecondInstance;
          return primary;
        }
      : undefined,
  };
  // Absent whole on Android rather than member by member: there is no backend to
  // read a flavour from, and the service that would replace it refuses a read
  // outright unless the application holds focus, which none of these signatures
  // can report apart from an empty clipboard.
  const clipboardText = hosted("__blitsenNativeClipboardRead");
  const clipboardWrite = hosted("__blitsenNativeClipboardWrite");
  const nativeClipboard = {
    readText: clipboardText ? () => __blitsenNativeClipboardRead("text") : undefined,
    readHtml: clipboardText ? () => __blitsenNativeClipboardRead("html") : undefined,
    readImage: hosted("__blitsenNativeClipboardReadImage")
      ? () => __blitsenNativeClipboardReadImage()
      : undefined,
    writeText: clipboardWrite
      ? text => { __blitsenNativeClipboardWrite("text", String(text)); }
      : undefined,
    writeHtml: clipboardWrite
      ? (html, alternative = "") => {
          __blitsenNativeClipboardWrite("html", String(html), String(alternative));
        }
      : undefined,
    writeImage: hosted("__blitsenNativeClipboardWriteImage")
      ? image => {
          __blitsenNativeClipboardWriteImage(String(image.width), String(image.height), image.data);
        }
      : undefined,
    clear: hosted("__blitsenNativeClipboardClear")
      ? () => { __blitsenNativeClipboardClear(); }
      : undefined,
  };
  // The window this run already opened. Its size and scale factor are not here:
  // `innerWidth`, `innerHeight`, `devicePixelRatio` and the `resize` event
  // already answer those, and a second answer that could disagree is worse than
  // none. What is new is the monitors — including the ones the window is not on.
  //
  // Each member is hosted separately rather than the module as a whole, because
  // which of them exists is `dom_bridge/native.rs`'s decision and not this
  // side's to anticipate. Android currently takes all of them — the monitor list
  // included, which looks like the survivor and is not: winit enumerates no
  // monitors there, so it would report a device with no display.
  const windowProperty = hosted("__blitsenNativeWindowSet");
  const windowReadback = hosted("__blitsenNativeWindowGet");
  const nativeWindow = {
    setSize: hosted("__blitsenNativeWindowResize")
      ? (width, height) => {
          __blitsenNativeWindowResize(String(width), String(height));
        }
      : undefined,
    setFullscreen: windowProperty
      ? on => { __blitsenNativeWindowSet("fullscreen", String(Boolean(on))); }
      : undefined,
    isFullscreen: windowReadback ? () => __blitsenNativeWindowGet("fullscreen") : undefined,
    setDecorations: windowProperty
      ? on => { __blitsenNativeWindowSet("decorations", String(Boolean(on))); }
      : undefined,
    isDecorated: windowReadback ? () => __blitsenNativeWindowGet("decorations") : undefined,
    setAlwaysOnTop: windowProperty
      ? on => { __blitsenNativeWindowSet("alwaysOnTop", String(Boolean(on))); }
      : undefined,
    setCursor: windowProperty
      ? cursor => { __blitsenNativeWindowSet("cursor", String(cursor)); }
      : undefined,
    setCursorVisible: windowProperty
      ? on => { __blitsenNativeWindowSet("cursorVisible", String(Boolean(on))); }
      : undefined,
    setCursorGrab: windowProperty
      ? mode => { __blitsenNativeWindowSet("cursorGrab", String(mode)); }
      : undefined,
    monitors: hosted("__blitsenNativeWindowMonitors")
      ? () => JSON.parse(__blitsenNativeWindowMonitors())
      : undefined,
  };

  // What machine this is. `navigator.hardwareConcurrency` is the only thing the
  // web has in this direction and it answers one deliberately-coarse number, so
  // none of this is a re-spelling of something standard.
  //
  // Each call samples: these are readings, not constants, and a monitor polls
  // them. `cpu().usage` in particular is the share of each core busy since the
  // previous call, so the first call is the exception — with no previous call
  // to measure from it reports a baseline against the counters' own origin,
  // which on Linux is the average since boot. Every call after it measures the
  // interval the caller chose.
  const nativeOs = {
    cpu: () => JSON.parse(__blitsenNativeOsCpu()),
    memory: () => JSON.parse(__blitsenNativeOsMemory()),
    storage: () => JSON.parse(__blitsenNativeOsStorage()),
    host: () => JSON.parse(__blitsenNativeOsHost()),
    locale: () => JSON.parse(__blitsenNativeOsLocale()),
  };

  // Dialogs. Promise-returning rather than blocking: the call arrives on the
  // thread that pumps the window, so waiting here would stop the application
  // painting for as long as the dialog is up. Nothing is lost by that — the
  // dialog is drawn by the desktop, in its own process, modal to our window —
  // and the frame loop keeps turning, which is also how the answer gets back.
  const nativeDialogPending = typeof __blitsenNativeDialogPending === "function"
    ? __blitsenNativeDialogPending : () => false;
  const dialogs = new Map();
  // The one handoff point for dialog answers, for the same reason `fetch` has
  // one: a promise must not settle part-way through a frame.
  const settleDialogs = () => {
    if (dialogs.size === 0) return;
    for (const closed of JSON.parse(__blitsenNativeDialogTake())) {
      const settle = dialogs.get(closed.id);
      if (!settle) continue;
      dialogs.delete(closed.id);
      settle(closed.value);
    }
  };
  const dialogInstalled = typeof __blitsenNativeDialogFile === "function";
  const opened = (id, answer) =>
    new Promise(resolve => { dialogs.set(id, value => resolve(answer(value))); });
  // Everything the options say is checked before the dialog opens, so a mistake
  // in the call is an exception where it was made rather than a promise that
  // rejects a frame later.
  const dialogOptions = options => {
    if (options === null || typeof options !== "object")
      throw new TypeError("dialog options must be an object");
    return options;
  };
  const fileDialog = (kind, options, answer) => {
    const { title, directory, fileName, filters = [] } = dialogOptions(options);
    if (!Array.isArray(filters)) throw new TypeError("dialog filters must be an array");
    return opened(__blitsenNativeDialogFile(kind, JSON.stringify({
      title: title === undefined ? null : String(title),
      directory: directory === undefined ? null : String(directory),
      fileName: fileName === undefined ? null : String(fileName),
      filters: filters.map(filter => {
        if (filter === null || typeof filter !== "object" || !Array.isArray(filter.extensions))
          throw new TypeError("a dialog filter is { name, extensions: [...] }");
        return { name: String(filter.name), extensions: filter.extensions.map(String) };
      }),
    })), answer);
  };
  // A cancelled dialog answers null rather than an empty selection: the portal
  // cannot tell us which one happened, and null is the one a caller must handle.
  const one = paths => paths[0] ?? null;
  const many = paths => (paths.length > 0 ? paths : null);
  // Every member is absent together: one platform decision installs the host
  // functions all of these need, or none of them.
  const filePicker = (kind, answer) => dialogInstalled
    ? (options = {}) => fileDialog(kind, options, answer)
    : undefined;
  const nativeDialog = {
    openFile: filePicker("openFile", one),
    openFiles: filePicker("openFiles", many),
    saveFile: filePicker("saveFile", one),
    openFolder: filePicker("openFolder", one),
    openFolders: filePicker("openFolders", many),
    message: !dialogInstalled ? undefined : (options = {}) => {
      const { title = "", message = "", level = "info", buttons = "ok" } = dialogOptions(options);
      return opened(__blitsenNativeDialogMessage(JSON.stringify({
        title: String(title), message: String(message),
        level: String(level), buttons: String(buttons),
      })), button => button);
    },
  };
  globalThis[Symbol.for("blitsen.native")] = Object.freeze({
    app: nativeMembers(nativeApp),
    clipboard: nativeMembers(nativeClipboard),
    window: nativeMembers(nativeWindow),
    os: nativeMembers(nativeOs),
    dialog: nativeMembers(nativeDialog),
  });


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
  const windowCommand = hosted("__blitsenNativeWindowCommand");
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
    setMinimized: windowProperty
      ? on => { __blitsenNativeWindowSet("minimized", String(Boolean(on))); }
      : undefined,
    setMaximized: windowProperty
      ? on => { __blitsenNativeWindowSet("maximized", String(Boolean(on))); }
      : undefined,
    isMaximized: windowReadback ? () => __blitsenNativeWindowGet("maximized") : undefined,
    startDrag: windowCommand
      ? () => { __blitsenNativeWindowCommand("startDrag"); }
      : undefined,
    close: windowCommand
      ? () => { __blitsenNativeWindowCommand("close"); }
      : undefined,
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

  // The session owns one tray. Package configuration supplies its startup
  // state; this module replaces or removes that same tray once JavaScript is
  // running, rather than creating an unrelated second icon.
  const trayInstalled = hosted("__blitsenNativeTrayConfigure");
  const trayCommands = new Map();
  const trayClickListeners = new Set();
  const trayActionListeners = new Set();
  const nativeTrayPending = hosted("__blitsenNativeTrayPending")
    ? __blitsenNativeTrayPending : () => false;
  const nativeTrayWorkPending = () => trayCommands.size > 0 || nativeTrayPending();
  const trayListener = (listeners, listener, name) => {
    if (typeof listener !== "function") throw new TypeError(`${name} listener must be a function`);
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  };
  const runTrayCommand = id => new Promise((resolve, reject) => {
    trayCommands.set(id, { resolve, reject });
  });
  const settleTrays = () => {
    if (!nativeTrayPending()) return;
    for (const message of JSON.parse(__blitsenNativeTrayTake())) {
      if (message.type === "completion") {
        const command = trayCommands.get(String(message.commandId));
        if (!command) continue;
        trayCommands.delete(String(message.commandId));
        if (message.error === null) command.resolve();
        else command.reject(new Error(message.error));
        continue;
      }
      const selected = message.type === "click" ? trayClickListeners : trayActionListeners;
      const event = message.type === "click"
        ? Object.freeze({ type: "click" })
        : Object.freeze(message.checked === undefined
          ? { type: "action", id: message.id }
          : { type: "action", id: message.id, checked: message.checked });
      for (const listener of selected) {
        try { listener(event); }
        catch (error) { console.error("Uncaught exception in tray listener", error); }
      }
    }
  };
  const normaliseTrayOptions = options => {
    if (options === null || typeof options !== "object")
      throw new TypeError("tray options must be an object");
    const { icon, tooltip = null, openOnClick = true, closeToTray = false, menu = [] } = options;
    if (!(icon instanceof Uint8Array) && !(icon instanceof Uint8ClampedArray))
      throw new TypeError("tray icon must be a Uint8Array or Uint8ClampedArray");
    if (!Array.isArray(menu)) throw new TypeError("tray menu must be an array");
    const menuIcons = [];
    let itemCount = 0;
    const nonEmpty = (value, description) => {
      if (value === undefined || String(value).length === 0)
        throw new TypeError(`${description} must be a non-empty string`);
      return String(value);
    };
    const accelerator = value => {
      if (value === undefined) return null;
      const result = nonEmpty(value, "tray accelerator");
      const parts = result.split("+").map(part => part.trim());
      const modifiers = new Set([
        "ctrl", "control", "alt", "option", "shift", "cmd", "command", "super", "meta",
        "cmdorctrl", "commandorcontrol",
      ]);
      if (parts.some(part => part.length === 0)
        || modifiers.has(parts[parts.length - 1].toLowerCase())
        || parts.slice(0, -1).some(part => !modifiers.has(part.toLowerCase()))
        || new Set(parts.slice(0, -1).map(part => part.toLowerCase())).size !== parts.length - 1)
        throw new TypeError(
          `invalid tray accelerator ${JSON.stringify(result)}: modifiers must precede one key`,
        );
      return result;
    };
    const itemIcon = value => {
      if (value === undefined) return null;
      if (!(value instanceof Uint8Array) && !(value instanceof Uint8ClampedArray))
        throw new TypeError("a tray menu icon must be a Uint8Array or Uint8ClampedArray");
      menuIcons.push(value);
      return menuIcons.length - 1;
    };
    const normaliseMenu = (items, depth = 1) => {
      if (!Array.isArray(items)) throw new TypeError("tray menu must be an array");
      if (depth > 16) throw new TypeError("tray menus may be nested at most 16 levels");
      return items.map(item => {
        if (++itemCount > 512) throw new TypeError("tray menus may contain at most 512 entries");
        if (item === null || typeof item !== "object")
          throw new TypeError("a tray menu item must be an object");
        const type = item.type === undefined
          ? (item.action === "separator" ? "separator" : "action")
          : String(item.type);
        if (type === "separator") return { type };
        if (type === "submenu") return {
          type,
          label: nonEmpty(item.label, "tray submenu label"),
          enabled: item.enabled === undefined ? true : Boolean(item.enabled),
          iconIndex: itemIcon(item.icon),
          menu: normaliseMenu(item.menu, depth + 1),
        };
        if (type === "checkbox" || type === "radio") return {
          type,
          id: nonEmpty(item.id, "checkable tray item id"),
          label: nonEmpty(item.label, "checkable tray item label"),
          enabled: item.enabled === undefined ? true : Boolean(item.enabled),
          checked: item.checked === undefined ? false : Boolean(item.checked),
          group: type === "radio" ? nonEmpty(item.group, "tray radio group") : null,
          accelerator: accelerator(item.accelerator),
        };
        if (type !== "action") throw new TypeError(`unknown tray menu item type: ${type}`);
        const hasId = item.id !== undefined;
        const hasAction = item.action !== undefined;
        if (hasId === hasAction)
          throw new TypeError("a tray action must have exactly one of id or action");
        return {
          type,
          id: hasId ? nonEmpty(item.id, "tray action id") : null,
          action: hasAction ? String(item.action) : null,
          label: item.label === undefined ? null : String(item.label),
          enabled: item.enabled === undefined ? true : Boolean(item.enabled),
          accelerator: accelerator(item.accelerator),
          iconIndex: itemIcon(item.icon),
        };
      });
    };
    return {
      icon,
      menuIcons,
      json: JSON.stringify({
        tooltip: tooltip === null ? null : String(tooltip),
        openOnClick: Boolean(openOnClick),
        closeToTray: Boolean(closeToTray),
        menu: normaliseMenu(menu),
      }),
    };
  };
  const nativeTray = {
    configure: !trayInstalled ? undefined : options => {
      const normalised = normaliseTrayOptions(options);
      return runTrayCommand(__blitsenNativeTrayConfigure(
        normalised.json, normalised.icon, ...normalised.menuIcons,
      ));
    },
    remove: !trayInstalled ? undefined : () => runTrayCommand(__blitsenNativeTrayRemove()),
    onClick: !trayInstalled ? undefined
      : listener => trayListener(trayClickListeners, listener, "tray click"),
    onAction: !trayInstalled ? undefined
      : listener => trayListener(trayActionListeners, listener, "tray action"),
  };

  // The application menu: the macOS main menu, and the Windows window menu bar.
  // Nothing here is the tray's — an application that shows no status item still
  // has one of these, and replacing one must not disturb the other. Both are
  // absent where the platform has none to install (`native-modules.mjs`).
  const menuInstalled = hosted("__blitsenNativeMenuConfigure");
  const menuCommands = new Map();
  const menuActionListeners = new Set();
  const nativeMenuPending = hosted("__blitsenNativeMenuPending")
    ? __blitsenNativeMenuPending : () => false;
  const nativeMenuWorkPending = () => menuCommands.size > 0 || nativeMenuPending();
  const runMenuCommand = id => new Promise((resolve, reject) => {
    menuCommands.set(id, { resolve, reject });
  });
  const settleMenus = () => {
    if (!nativeMenuPending()) return;
    for (const message of JSON.parse(__blitsenNativeMenuTake())) {
      if (message.type === "completion") {
        const command = menuCommands.get(String(message.commandId));
        if (!command) continue;
        menuCommands.delete(String(message.commandId));
        if (message.error === null) command.resolve();
        else command.reject(new Error(message.error));
        continue;
      }
      const event = Object.freeze(message.checked === undefined
        ? { type: "action", id: message.id }
        : { type: "action", id: message.id, checked: message.checked });
      for (const listener of menuActionListeners) {
        try { listener(event); }
        catch (error) { console.error("Uncaught exception in menu listener", error); }
      }
    }
  };
  // The same tree the tray accepts, minus the entries a menu bar cannot carry
  // and plus the roles only it can: the host parses one model for both, and
  // this is the half of that contract the caller hears about immediately.
  const normaliseMenuEntries = options => {
    if (options === null || typeof options !== "object")
      throw new TypeError("application menu options must be an object");
    const { menu } = options;
    if (!Array.isArray(menu)) throw new TypeError("the application menu must be an array");
    let itemCount = 0;
    const nonEmpty = (value, description) => {
      if (value === undefined || String(value).length === 0)
        throw new TypeError(`${description} must be a non-empty string`);
      return String(value);
    };
    const accelerator = value => {
      if (value === undefined) return null;
      const result = nonEmpty(value, "menu accelerator");
      const parts = result.split("+").map(part => part.trim());
      const modifiers = new Set([
        "ctrl", "control", "alt", "option", "shift", "cmd", "command", "super", "meta",
        "cmdorctrl", "commandorcontrol",
      ]);
      if (parts.some(part => part.length === 0)
        || modifiers.has(parts[parts.length - 1].toLowerCase())
        || parts.slice(0, -1).some(part => !modifiers.has(part.toLowerCase()))
        || new Set(parts.slice(0, -1).map(part => part.toLowerCase())).size !== parts.length - 1)
        throw new TypeError(
          `invalid menu accelerator ${JSON.stringify(result)}: modifiers must precede one key`,
        );
      return result;
    };
    const normaliseLevel = (items, depth = 1) => {
      if (!Array.isArray(items)) throw new TypeError("an application menu must be an array");
      if (depth > 16) throw new TypeError("application menus may be nested at most 16 levels");
      return items.map(item => {
        if (++itemCount > 512)
          throw new TypeError("application menus may contain at most 512 entries");
        if (item === null || typeof item !== "object")
          throw new TypeError("an application menu item must be an object");
        const type = item.type === undefined ? "action" : String(item.type);
        if (depth === 1 && type !== "submenu")
          throw new TypeError("every top-level application menu entry must be a submenu");
        if (type === "separator") return { type };
        if (type === "role") return { type, role: nonEmpty(item.role, "an application menu role") };
        if (type === "submenu") return {
          type,
          label: nonEmpty(item.label, "application submenu label"),
          role: item.role === undefined ? null : nonEmpty(item.role, "an application submenu role"),
          enabled: item.enabled === undefined ? true : Boolean(item.enabled),
          menu: normaliseLevel(item.menu, depth + 1),
        };
        if (type === "checkbox" || type === "radio") return {
          type,
          id: nonEmpty(item.id, "checkable menu item id"),
          label: nonEmpty(item.label, "checkable menu item label"),
          enabled: item.enabled === undefined ? true : Boolean(item.enabled),
          checked: item.checked === undefined ? false : Boolean(item.checked),
          group: type === "radio" ? nonEmpty(item.group, "menu radio group") : null,
          accelerator: accelerator(item.accelerator),
        };
        if (type !== "action") throw new TypeError(`unknown application menu item type: ${type}`);
        return {
          type,
          id: nonEmpty(item.id, "application menu action id"),
          label: nonEmpty(item.label, "application menu action label"),
          enabled: item.enabled === undefined ? true : Boolean(item.enabled),
          accelerator: accelerator(item.accelerator),
        };
      });
    };
    return JSON.stringify(normaliseLevel(menu));
  };
  const nativeMenu = {
    configure: !menuInstalled ? undefined : options =>
      runMenuCommand(__blitsenNativeMenuConfigure(normaliseMenuEntries(options))),
    remove: !menuInstalled ? undefined : () => runMenuCommand(__blitsenNativeMenuRemove()),
    onAction: !menuInstalled ? undefined : listener => {
      if (typeof listener !== "function")
        throw new TypeError("menu action listener must be a function");
      menuActionListeners.add(listener);
      return () => { menuActionListeners.delete(listener); };
    },
  };

  // Polling state for games and other frame-oriented applications. Ordinary
  // interaction remains DOM keyboard, pointer and wheel events; a snapshot is
  // the held-state and raw-relative complement to those events.
  const nativeInput = {
    snapshot: hosted("__blitsenNativeInputSnapshot")
      ? () => Object.freeze(JSON.parse(__blitsenNativeInputSnapshot()))
      : undefined,
    vibrateGamepad: gamepadInstalled
      ? (index, options = {}) => {
          const slot = Number(index);
          const duration = Number(options.duration ?? 0);
          const strong = Number(options.strongMagnitude ?? 0);
          const weak = Number(options.weakMagnitude ?? 0);
          if (!Number.isSafeInteger(slot) || slot < 0
            || ![duration, strong, weak].every(Number.isFinite)
            || duration < 0 || duration > 60_000
            || strong < 0 || strong > 1 || weak < 0 || weak > 1)
            return Promise.reject(new TypeError(
              "gamepad index must be non-negative, duration 0..60000ms, and magnitudes 0..1"));
          return startGamepadVibration(slot, strong, weak, duration);
        }
      : undefined,
    onDeviceChange: gamepadInstalled ? gamepadListener : undefined,
  };

  // Raw HID (#247). Deliberately not part of `input` above: keyboards, pointers
  // and controllers are DOM events and the Gamepad API, and raw reports are a
  // separate capability with a separate security boundary (S10).
  //
  // Which devices exist is the host's answer and nothing here can widen it —
  // the Generic Desktop keyboard, keypad, mouse and pointer collections are
  // gone before `devices()` resolves, and the ids that survive name nothing
  // about the machine. Every call settles on a frame turn, because a report is
  // read by a native worker that owns the handle and must never re-enter the
  // application from the thread it blocked on.
  const hidInstalled = hosted("__blitsenNativeHidDevices");
  const hidCommands = new Map();
  const hidOpenDevices = new Map();
  const hidChangeListeners = new Set();
  const nativeHidPending = hosted("__blitsenNativeHidPending")
    ? __blitsenNativeHidPending : () => false;
  // An open device and a hot-plug listener both keep the loop turning, for the
  // reason a live socket does: the report is already in the host, and a loop
  // that idled would never reach the turn that delivers it.
  const nativeHidWorkPending = () => hidCommands.size > 0 || hidOpenDevices.size > 0
    || hidChangeListeners.size > 0 || nativeHidPending();
  const runHidCommand = id => new Promise((resolve, reject) => {
    hidCommands.set(String(id), { resolve, reject });
  });
  const hidListener = (listeners, listener, what) => {
    if (typeof listener !== "function") throw new TypeError(`${what} listener must be a function`);
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  };
  const deliverHid = (listeners, event, what) => {
    for (const listener of listeners) {
      try { listener(event); }
      catch (error) { console.error(`Uncaught exception in ${what} listener`, error); }
    }
  };
  const hidDeviceInfo = info => Object.freeze({
    ...info, usages: Object.freeze(info.usages.map(usage => Object.freeze(usage))),
  });
  const settleHid = () => {
    if (!nativeHidPending()) return;
    for (const { json, data } of __blitsenNativeHidTake()) {
      const message = JSON.parse(json);
      if (message.type === "completion") {
        const command = hidCommands.get(String(message.commandId));
        if (!command) continue;
        hidCommands.delete(String(message.commandId));
        // The four open outcomes are told apart by the exception name rather
        // than by its text, so `error.name === "NotAllowedError"` is a udev
        // rule or an entitlement and nothing else is.
        if (message.error !== null)
          command.reject(new DOMException(message.error, message.errorName));
        else command.resolve(data === null ? message.value : data);
        continue;
      }
      if (message.type === "change") {
        deliverHid(hidChangeListeners, Object.freeze({
          type: message.change, device: hidDeviceInfo(message.device),
        }), "HID device change");
        continue;
      }
      const device = hidOpenDevices.get(message.deviceId);
      if (!device) continue;
      if (message.type === "input") {
        // The report ID is separate and the data excludes it, so nothing here
        // depends on whether a platform backend retained the leading byte.
        deliverHid(device.inputListeners, Object.freeze({
          deviceId: message.deviceId, reportId: message.reportId, data,
        }), "HID input report");
        continue;
      }
      // The one terminal event. The host has already closed the handle and
      // will not send another for this device.
      hidOpenDevices.delete(message.deviceId);
      device.opened = false;
      deliverHid(device.disconnectListeners, Object.freeze({ deviceId: message.deviceId }),
        "HID disconnect");
    }
  };
  // Checked here, at the call, rather than a frame later in a rejection: the
  // application knows the bound because `open` reported it, so a report past it
  // is a mistake in this line of code.
  const hidReport = (data, limit, what) => {
    if (!(data instanceof Uint8Array) && !(data instanceof Uint8ClampedArray))
      throw new TypeError(`a HID ${what} must be a Uint8Array or Uint8ClampedArray`);
    if (data.length === 0) throw new TypeError(`a HID ${what} needs at least the report ID byte`);
    if (data.length > limit)
      throw new TypeError(
        `a HID ${what} of ${data.length} bytes exceeds the ${limit} this device declared`);
    return data;
  };
  const hidDevice = (id, opened) => {
    const state = {
      opened: true,
      maxInputReportSize: Number(opened.maxInputReportSize),
      maxOutputReportSize: Number(opened.maxOutputReportSize),
      maxFeatureReportSize: Number(opened.maxFeatureReportSize),
      inputListeners: new Set(),
      disconnectListeners: new Set(),
    };
    hidOpenDevices.set(id, state);
    const live = () => {
      if (!state.opened) throw new DOMException(`HID device ${id} is closed`, "InvalidStateError");
    };
    return Object.freeze({
      id,
      info: hidDeviceInfo(opened.device),
      get opened() { return state.opened; },
      maxInputReportSize: state.maxInputReportSize,
      maxOutputReportSize: state.maxOutputReportSize,
      maxFeatureReportSize: state.maxFeatureReportSize,
      write: data => {
        live();
        return runHidCommand(__blitsenNativeHidWrite(
          id, hidReport(data, state.maxOutputReportSize, "output report")));
      },
      sendFeatureReport: data => {
        live();
        return runHidCommand(__blitsenNativeHidSendFeatureReport(
          id, hidReport(data, state.maxFeatureReportSize, "feature report")));
      },
      receiveFeatureReport: reportId => {
        live();
        const report = Number(reportId);
        if (!Number.isInteger(report) || report < 0 || report > 0xff)
          throw new TypeError("a HID report id is a byte");
        return runHidCommand(__blitsenNativeHidReceiveFeatureReport(id, String(report)));
      },
      onInputReport: listener => hidListener(state.inputListeners, listener, "HID input report"),
      onDisconnect: listener => hidListener(state.disconnectListeners, listener, "HID disconnect"),
      close: () => {
        if (!state.opened) return Promise.resolve(null);
        state.opened = false;
        hidOpenDevices.delete(id);
        // A device unplugged in the same turn this was called is already closed
        // in the host, which answers that a device it does not have open cannot
        // be closed. The application asked for the state it now has, and could
        // not have avoided the race, so this resolves rather than rejecting.
        return runHidCommand(__blitsenNativeHidClose(id)).catch(() => null);
      },
    });
  };
  const nativeHid = {
    devices: !hidInstalled ? undefined : () => runHidCommand(__blitsenNativeHidDevices())
      .then(found => Object.freeze(found.map(hidDeviceInfo))),
    open: !hidInstalled ? undefined : deviceId => {
      const id = String(deviceId);
      if (hidOpenDevices.has(id))
        throw new DOMException(`HID device ${id} is already open`, "InvalidStateError");
      return runHidCommand(__blitsenNativeHidOpen(id)).then(opened => hidDevice(id, opened));
    },
    // The host polls for hot-plug, so it is told when anything is listening and
    // told again when the last listener goes: an application that never asks
    // never makes the runtime walk the device tree.
    onDeviceChange: !hidInstalled ? undefined : listener => {
      const remove = hidListener(hidChangeListeners, listener, "HID device change");
      __blitsenNativeHidWatch(true);
      return () => {
        remove();
        if (hidChangeListeners.size === 0) __blitsenNativeHidWatch(false);
      };
    },
  };

  // Desktop notification commands settle at the top of a frame. Platform
  // callbacks only enqueue messages in the host; no notification service or
  // callback thread is allowed to enter application JavaScript directly.
  const notifyInstalled = hosted("__blitsenNativeNotifyShow");
  const notifyCommands = new Map();
  const notifyListeners = new Set();
  const nativeNotifyPending = hosted("__blitsenNativeNotifyPending")
    ? __blitsenNativeNotifyPending : () => false;
  const nativeNotifyWorkPending = () => notifyCommands.size > 0 || nativeNotifyPending();
  const runNotifyCommand = (id, onComplete = null) => new Promise((resolve, reject) => {
    notifyCommands.set(String(id), { resolve, reject, onComplete });
  });
  const settleNotifications = () => {
    if (!nativeNotifyPending()) return;
    for (const message of JSON.parse(__blitsenNativeNotifyTake())) {
      if (message.type === "completion") {
        const command = notifyCommands.get(String(message.commandId));
        if (!command) continue;
        notifyCommands.delete(String(message.commandId));
        command.onComplete?.(message.value, message.error);
        if (message.error === null) command.resolve(message.value);
        else command.reject(new Error(message.error));
        continue;
      }
      const event = Object.freeze(message);
      settleStandardNotification?.(event);
      for (const listener of notifyListeners) {
        try { listener(event); }
        catch (error) { console.error("Uncaught exception in notification listener", error); }
      }
    }
  };
  const normaliseNotificationActions = actions => {
    if (actions === undefined) return [];
    if (!Array.isArray(actions)) throw new TypeError("notification actions must be an array");
    if (actions.length > 8) throw new TypeError("notifications may contain at most 8 actions");
    const ids = new Set();
    return actions.map(action => {
      if (action === null || typeof action !== "object")
        throw new TypeError("a notification action must be an object");
      const id = String(action.id ?? "");
      const title = String(action.title ?? "");
      if (!id || !title) throw new TypeError("notification action ids and titles must not be empty");
      if (id === "default" || ids.has(id))
        throw new TypeError(`notification action id ${JSON.stringify(id)} is reserved or duplicated`);
      ids.add(id);
      return { id, title };
    });
  };
  const notificationTimeout = value => {
    const timeout = Number(value);
    if (!Number.isFinite(timeout) || timeout < 0 || timeout > 0xffffffff)
      throw new TypeError(
        "notification timeout must be a non-negative 32-bit number of milliseconds",
      );
    return Math.trunc(timeout);
  };
  const notificationUrgency = value => {
    const urgency = String(value);
    if (!new Set(["low", "normal", "critical"]).has(urgency))
      throw new TypeError(`${JSON.stringify(urgency)} is not a notification urgency`);
    return urgency;
  };
  const normaliseNotification = options => {
    if (options === null || typeof options !== "object")
      throw new TypeError("notification options must be an object");
    if (options.title === undefined || String(options.title).length === 0)
      throw new TypeError("a notification needs a non-empty title");
    return {
      title: String(options.title),
      body: options.body === undefined ? "" : String(options.body),
      appName: options.appName === undefined ? null : String(options.appName),
      timeout: options.timeout === undefined ? null : notificationTimeout(options.timeout),
      urgency: options.urgency === undefined ? "normal" : notificationUrgency(options.urgency),
      icon: options.icon === undefined ? null : String(options.icon),
      actions: normaliseNotificationActions(options.actions),
    };
  };
  const normaliseNotificationUpdate = options => {
    if (options === null || typeof options !== "object")
      throw new TypeError("notification update must be an object");
    const update = {};
    if (options.title !== undefined) {
      update.title = String(options.title);
      if (!update.title) throw new TypeError("a notification title must not be empty");
    }
    if (options.body !== undefined) update.body = String(options.body);
    if (options.appName !== undefined) update.appName = String(options.appName);
    if (options.timeout !== undefined) update.timeout = notificationTimeout(options.timeout);
    if (options.urgency !== undefined) update.urgency = notificationUrgency(options.urgency);
    if (options.icon !== undefined) update.icon = String(options.icon);
    if (options.actions !== undefined) update.actions = normaliseNotificationActions(options.actions);
    return update;
  };
  const nativeNotify = {
    show: !notifyInstalled ? undefined : options => runNotifyCommand(
      __blitsenNativeNotifyShow(JSON.stringify(normaliseNotification(options))),
    ),
    permission: !notifyInstalled ? undefined : async () => __blitsenNativeNotifyPermission(),
    requestPermission: !notifyInstalled ? undefined : () => runNotifyCommand(
      __blitsenNativeNotifyRequestPermission(),
    ),
    update: !notifyInstalled ? undefined : (id, options) => runNotifyCommand(
      __blitsenNativeNotifyUpdate(String(id), JSON.stringify(normaliseNotificationUpdate(options))),
    ),
    close: !notifyInstalled ? undefined : id => runNotifyCommand(
      __blitsenNativeNotifyClose(String(id)),
    ),
    onEvent: !notifyInstalled ? undefined : listener => {
      if (typeof listener !== "function")
        throw new TypeError("notification event listener must be a function");
      notifyListeners.add(listener);
      return () => { notifyListeners.delete(listener); };
    },
  };

  // The standard constructor is installed only when close is addressable. The
  // current Windows library backend cannot satisfy that contract, so feature
  // detection stays honest there while `blitsen/notify` remains available.
  const standardNotifications = new Map();
  const standardNotificationStates = new WeakMap();
  const standardNotificationState = notification => {
    const state = standardNotificationStates.get(notification);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };
  let settleStandardNotification = null;
  const Notification = !hosted("__blitsenNativeNotifyStandard") ? undefined : class Notification
    extends EventTarget {
    constructor(title, options = {}) {
      super();
      if (options === null || typeof options !== "object")
        throw new TypeError("Notification options must be an object");
      const unsupported = [];
      if (options.tag !== undefined && String(options.tag)) unsupported.push("tag replacement");
      if (options.image !== undefined && String(options.image)) unsupported.push("image");
      if (options.badge !== undefined && String(options.badge)) unsupported.push("badge");
      if (options.vibrate !== undefined
        && (Array.isArray(options.vibrate) ? options.vibrate.length > 0 : Number(options.vibrate) !== 0))
        unsupported.push("vibration");
      if (Boolean(options.renotify)) unsupported.push("renotify");
      if (Boolean(options.silent)) unsupported.push("silent delivery");
      if (unsupported.length > 0)
        throw new DOMException(
          `Notification does not support ${unsupported.join(", ")} on this runtime`,
          "NotSupportedError",
        );
      const state = {
        id: null,
        closed: false,
        handlers: {},
        title: String(title),
        dir: String(options.dir ?? "auto"),
        lang: String(options.lang ?? ""),
        body: String(options.body ?? ""),
        tag: "",
        icon: String(options.icon ?? ""),
        badge: "",
        image: "",
        requireInteraction: Boolean(options.requireInteraction),
        silent: false,
        timestamp: Number(options.timestamp ?? Date.now()),
        vibrate: [],
        data: options.data === undefined ? null : structuredClone(options.data),
        actions: (options.actions ?? []).map(action => Object.freeze({
          action: String(action.action ?? ""),
          title: String(action.title ?? ""),
          icon: String(action.icon ?? ""),
        })),
      };
      if (!state.title) throw new TypeError("a Notification needs a non-empty title");
      if (!new Set(["auto", "ltr", "rtl"]).has(state.dir))
        throw new TypeError(`Notification dir must be auto, ltr or rtl, not ${state.dir}`);
      if (state.actions.some(action => !action.action || !action.title || action.icon))
        throw new DOMException(
          "Notification actions need an action and title; per-action icons are unsupported",
          "NotSupportedError",
        );
      Object.freeze(state.actions);
      standardNotificationStates.set(this, state);
      const nativeOptions = {
        title: state.title,
        body: state.body,
        icon: state.icon || undefined,
        timeout: state.requireInteraction ? 0 : undefined,
        actions: state.actions.map(action => ({ id: action.action, title: action.title })),
      };
      const command = __blitsenNativeNotifyShow(
        JSON.stringify(normaliseNotification(nativeOptions)),
      );
      void runNotifyCommand(command, (id, error) => {
        if (error !== null) {
          if (!state.closed) {
            state.closed = true;
            this.dispatchEvent(new Event("error"));
          }
          return;
        }
        if (state.closed) {
          state.id = String(id);
          standardNotifications.set(state.id, this);
          void nativeNotify.close(state.id).catch(() => {});
          return;
        }
        state.id = String(id);
        standardNotifications.set(state.id, this);
      }).catch(() => {});
    }
    static get permission() { return __blitsenNativeNotifyPermission(); }
    static requestPermission(callback = undefined) {
      if (callback !== undefined && typeof callback !== "function")
        throw new TypeError("Notification permission callback must be a function");
      const result = nativeNotify.requestPermission();
      if (callback) void result.then(callback);
      return result;
    }
    static get maxActions() { return 8; }
    get title() { return standardNotificationState(this).title; }
    get dir() { return standardNotificationState(this).dir; }
    get lang() { return standardNotificationState(this).lang; }
    get body() { return standardNotificationState(this).body; }
    get tag() { return standardNotificationState(this).tag; }
    get icon() { return standardNotificationState(this).icon; }
    get badge() { return standardNotificationState(this).badge; }
    get image() { return standardNotificationState(this).image; }
    get requireInteraction() { return standardNotificationState(this).requireInteraction; }
    get silent() { return standardNotificationState(this).silent; }
    get timestamp() { return standardNotificationState(this).timestamp; }
    get vibrate() { return standardNotificationState(this).vibrate; }
    get data() { return standardNotificationState(this).data; }
    get actions() { return standardNotificationState(this).actions; }
    get onclick() { return standardNotificationState(this).handlers.onclick ?? null; }
    set onclick(callback) {
      const state = standardNotificationState(this);
      setEventHandler(this, state.handlers, "click", callback, "onclick");
    }
    get onshow() { return standardNotificationState(this).handlers.onshow ?? null; }
    set onshow(callback) {
      const state = standardNotificationState(this);
      setEventHandler(this, state.handlers, "show", callback, "onshow");
    }
    get onerror() { return standardNotificationState(this).handlers.onerror ?? null; }
    set onerror(callback) {
      const state = standardNotificationState(this);
      setEventHandler(this, state.handlers, "error", callback, "onerror");
    }
    get onclose() { return standardNotificationState(this).handlers.onclose ?? null; }
    set onclose(callback) {
      const state = standardNotificationState(this);
      setEventHandler(this, state.handlers, "close", callback, "onclose");
    }
    close() {
      const state = standardNotificationState(this);
      if (state.closed) return;
      state.closed = true;
      if (state.id === null) return;
      void nativeNotify.close(state.id).catch(() => {});
    }
  };
  if (Notification) settleStandardNotification = message => {
    const notification = standardNotifications.get(String(message.id));
    if (!notification) return;
    const state = standardNotificationState(notification);
    if (message.type === "show") {
      if (!state.closed) notification.dispatchEvent(new Event("show"));
      return;
    }
    standardNotifications.delete(state.id);
    state.closed = true;
    if (message.type === "click" || message.type === "action")
      notification.dispatchEvent(new Event("click"));
    else if (message.type === "error") notification.dispatchEvent(new Event("error"));
    else if (message.type === "close") notification.dispatchEvent(new Event("close"));
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
  //
  // `batteries` is the one member here that is hosted separately: it is the one
  // Android has no backend for, and an empty list there would claim a phone runs
  // on mains. Everywhere else an empty list is the machine's own answer — a
  // desktop has no battery — and a machine that cannot be asked throws instead.
  const nativeOs = {
    cpu: () => JSON.parse(__blitsenNativeOsCpu()),
    memory: () => JSON.parse(__blitsenNativeOsMemory()),
    storage: () => JSON.parse(__blitsenNativeOsStorage()),
    host: () => JSON.parse(__blitsenNativeOsHost()),
    locale: () => JSON.parse(__blitsenNativeOsLocale()),
    batteries: hosted("__blitsenNativeOsBatteries")
      ? () => JSON.parse(__blitsenNativeOsBatteries())
      : undefined,
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
    tray: nativeMembers(nativeTray),
    menu: nativeMembers(nativeMenu),
    input: nativeMembers(nativeInput),
    hid: nativeMembers(nativeHid),
    notify: nativeMembers(nativeNotify),
    os: nativeMembers(nativeOs),
    dialog: nativeMembers(nativeDialog),
  });

  // Clipboard events and drag and drop: the two ways a `DataTransfer` reaches
  // an application, and the one place this runtime diverges from the web on
  // purpose.
  //
  // A browser answers a file drop with a `File` — an opaque handle whose bytes
  // are read back asynchronously — because a page must never learn where a user
  // keeps their files. An exported Blitsen application *is* the user's program,
  // and the platform already told the host the absolute path, so hiding it would
  // be inventing a restriction rather than honouring one. `DataTransfer.paths`
  // is that list, and `files`, `items` and `setDragImage` are absent rather than
  // approximated: there is no `File` in this runtime to put in one, and
  // `doctor` reports the three by name so an application that reads them is told
  // what to read instead (PRODUCT.md §7).
  //
  // The classes live together rather than with the other events because they are
  // one feature: the object, and the two events that exist to carry it.
  const transferStores = new WeakMap();
  const transferStore = transfer => {
    const store = transferStores.get(transfer);
    if (!store) throw new TypeError("Illegal invocation");
    return store;
  };
  // The two legacy format names HTML still requires be understood, and the
  // lowercasing every other one goes through.
  const TRANSFER_FORMAT_ALIASES = { text: "text/plain", url: "text/uri-list" };
  const transferFormat = format => {
    const name = String(format).toLowerCase();
    return TRANSFER_FORMAT_ALIASES[name] ?? name;
  };

  class DataTransfer {
    constructor() {
      transferStores.set(this,
        { data: new Map(), paths: Object.freeze([]), readOnly: false,
          dropEffect: "none", effectAllowed: "uninitialized" });
    }
    get dropEffect() { return transferStore(this).dropEffect; }
    set dropEffect(value) {
      const effect = String(value);
      if (["none", "copy", "link", "move"].includes(effect))
        transferStore(this).dropEffect = effect;
    }
    get effectAllowed() { return transferStore(this).effectAllowed; }
    set effectAllowed(value) { transferStore(this).effectAllowed = String(value); }
    // `Files` is among the types when the drag carries any, which is the check
    // an application makes before it accepts a drop. What follows it is
    // Blitsen's answer to "which files", and it is a path rather than a handle.
    get types() {
      const store = transferStore(this);
      return Object.freeze(store.paths.length > 0
        ? [...store.data.keys(), "Files"] : [...store.data.keys()]);
    }
    /** Absolute filesystem paths, in the order the platform listed them. */
    get paths() { return transferStore(this).paths; }
    getData(format) { return transferStore(this).data.get(transferFormat(format)) ?? ""; }
    // A drag or a paste hands the application a store it may read and not
    // rewrite, which HTML calls read-only mode and spells as these two doing
    // nothing rather than as a thrown error.
    setData(format, data) {
      const store = transferStore(this);
      if (!store.readOnly) store.data.set(transferFormat(format), String(data));
    }
    clearData(format) {
      const store = transferStore(this);
      if (store.readOnly) return;
      if (format === undefined) store.data.clear();
      else store.data.delete(transferFormat(format));
    }
  }

  class ClipboardEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      // Null on an event the application constructed itself, as in a browser:
      // only a clipboard action the platform started has a store behind it.
      defineMembers(this, { clipboardData: options.clipboardData ?? null });
    }
  }

  class DragEvent extends MouseEvent {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, { dataTransfer: options.dataTransfer ?? null });
    }
  }

  // The drag currently over this window: its data store, the element it is over,
  // and whether that element agreed to take it.
  //
  // The store outlives each event because one drag is one store — winit names
  // the files when the drag arrives and again when it is released, and never in
  // between — and the element is here rather than in the host for the reason
  // pointer capture is: only this side knows which node the last event reached.
  let dragTransfer = null;
  let dragTarget = null;
  let dragAccepted = false;

  const dragMembers = (init, transfer) => ({
    ...init, view: globalThis, bubbles: true, cancelable: true,
    // A drag is not a press: no button changed and none is held, which is what
    // a browser reports on every one of these events.
    button: 0, buttons: 0, dataTransfer: transfer,
  });

  // Refreshes the session's store from what the host just reported.
  const dragSessionTransfer = init => {
    if (dragTransfer === null) dragTransfer = new DataTransfer();
    const store = transferStore(dragTransfer);
    store.data.clear();
    store.paths = Object.freeze(init.paths.map(String));
    // `text/uri-list` is CRLF-separated by its own registration, and the host
    // built each URL with the same parser `location` uses rather than pasting
    // `file://` in front of a path that may contain a space.
    if (init.uris.length > 0) store.data.set("text/uri-list", init.uris.join("\r\n"));
    store.readOnly = true;
    return dragTransfer;
  };

  // Tells the element the drag has left that it has, so the highlight it put up
  // on `dragenter` comes down. Not cancelable: by the time it is delivered the
  // drag is already somewhere else.
  const dragLeaveTarget = members => {
    const target = dragTarget;
    dragTarget = null;
    dragAccepted = false;
    target?.dispatchEvent(new DragEvent("dragleave",
      { ...members, cancelable: false, dataTransfer: dragTransfer }));
  };

  // One stage of a drag, as the DOM sequence it stands for.
  //
  // `drop` is dispatched only where the last `dragover` was cancelled. That is
  // HTML's own rule rather than a restriction added here: cancelling `dragover`
  // is how an element says it will take the drop, and an element that never
  // said so gets the browser's default action instead — which in a document
  // with nowhere to navigate to is nothing at all.
  const dispatchDragEvent = (stage, rawHandle, init) => {
    if (stage === "leave") {
      // No coordinates: a drag that has left the window is not anywhere in it,
      // and Windows reports no position for the crossing at all.
      dragLeaveTarget({ view: globalThis, bubbles: true });
      dragTransfer = null;
      return false;
    }
    const target = wrap(String(rawHandle));
    const members = dragMembers(init, dragSessionTransfer(init));
    if (dragTarget !== target) {
      dragLeaveTarget(members);
      dragTarget = target;
      target.dispatchEvent(new DragEvent("dragenter", members));
    }
    dragAccepted = !target.dispatchEvent(new DragEvent("dragover", members));
    if (stage !== "drop") return dragAccepted;
    const dropped = dragAccepted && !target.dispatchEvent(new DragEvent("drop", members));
    // A drop ends the session without a `dragleave`: the drag is over, and the
    // element that took it is not being left.
    dragTarget = null;
    dragAccepted = false;
    dragTransfer = null;
    return dropped;
  };

  const disposeDragState = () => {
    dragTransfer = null;
    dragTarget = null;
    dragAccepted = false;
  };

  // The clipboard events, and the shortcuts that raise them.
  //
  // The whole family needs the platform clipboard both to fill a `paste` and to
  // take what a `copy` produced, so where `dom_bridge/native.rs` installs no
  // clipboard — Android, which has no `arboard` backend — the shortcut does
  // nothing rather than dispatching an event with nothing behind it. The classes
  // above still exist there: an application may construct and dispatch its own.
  const clipboardEventsHosted = () =>
    hosted("__blitsenNativeClipboardRead") && hosted("__blitsenNativeClipboardWrite");
  const CLIPBOARD_SHORTCUTS = { c: "copy", x: "cut", v: "paste" };

  // What a `copy` or a `cut` puts on the clipboard when nothing intervened: the
  // selection inside the focused control, or the document selection when focus
  // is not in one.
  // A clipboard the platform refuses — a session with no selection owner, a
  // pasteboard another process is holding — is reported and not thrown. This is
  // the default action of a keystroke, and taking the document down because one
  // could not reach the clipboard is worse than the keystroke doing nothing.
  const clipboardAttempt = (description, action) => {
    try { return action(); }
    catch (error) { console.error(`The system clipboard could not be ${description}`, error); }
    return null;
  };

  const selectedText = target => {
    if (textControl(target) === null) return String(getSelection());
    const { start, end } = call("formSelection", target[handle]);
    return target.value.slice(start, end);
  };

  const writeClipboardStore = store => {
    const html = store.data.get("text/html");
    const text = store.data.get("text/plain") ?? "";
    if (html !== undefined)
      return clipboardAttempt("written", () => __blitsenNativeClipboardWrite("html", html, text));
    if (text !== "")
      return clipboardAttempt("written", () => __blitsenNativeClipboardWrite("text", text));
    return null;
  };

  // A clipboard action, announced to the application before it happens. A
  // cancelled `copy` or `cut` still writes — what it writes is whatever the
  // listener put in `clipboardData`, which is the whole reason that object is
  // writable on those two and read-only on `paste`.
  const dispatchClipboardEvent = (type, target) => {
    const transfer = new DataTransfer();
    const store = transferStore(transfer);
    if (type === "paste") {
      const text = clipboardAttempt("read", () => __blitsenNativeClipboardRead("text"));
      if (text !== null) store.data.set("text/plain", String(text));
      const html = clipboardAttempt("read", () => __blitsenNativeClipboardRead("html"));
      if (html !== null) store.data.set("text/html", String(html));
      store.readOnly = true;
    }
    const allowed = target.dispatchEvent(new ClipboardEvent(type,
      { bubbles: true, cancelable: true, clipboardData: transfer }));
    if (type === "paste") {
      const text = store.data.get("text/plain");
      if (!allowed || text === undefined || editableControl(target) === null) return;
      applyTextEdit(target, { operation: "insert", data: text, inputType: "insertFromPaste" });
      return;
    }
    if (allowed) {
      const selection = selectedText(target);
      if (selection === "") return;
      // The selection replaces the store rather than joining it: an event
      // nobody cancelled copies what is selected, and a flavour a listener left
      // behind would otherwise be pasted in its place.
      store.data.clear();
      store.data.set("text/plain", selection);
    }
    writeClipboardStore(store);
    // Only a `cut` the application let through removes anything, and only from
    // a control that is editable — a cut out of a read-only field is a copy.
    if (type === "cut" && allowed && editableControl(target) !== null)
      applyTextEdit(target, { operation: "insert", inputType: "deleteByCut" });
  };

  // The default action of a keydown that was one of the three clipboard
  // shortcuts, reporting whether it was: a key the clipboard took is not also a
  // character to type or a document to scroll.
  //
  // Either command modifier is accepted, because the platform this runs on
  // decides which one the user pressed and both spell the same intent. Shift
  // and Alt are not, so `Ctrl+Shift+C` stays the application's to bind.
  const clipboardShortcut = (event, target) => {
    if (event.shiftKey || event.altKey || !(event.ctrlKey || event.metaKey)) return false;
    const type = CLIPBOARD_SHORTCUTS[event.key.toLowerCase()];
    if (type === undefined || !clipboardEventsHosted()) return false;
    dispatchClipboardEvent(type, target);
    return true;
  };

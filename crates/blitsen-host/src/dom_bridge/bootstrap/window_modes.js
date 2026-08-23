  // One short-lived activation token, granted by native pointer/key dispatch.
  // Pointer lock requires it; fullscreen additionally consumes it, matching the
  // ordering that lets one gesture request pointer lock before fullscreen.
  let windowModeActivation = false;
  let activationGeneration = 0;
  const grantWindowModeActivation = () => {
    windowModeActivation = true;
    const generation = ++activationGeneration;
    queueMicrotask(() => {
      if (activationGeneration === generation) windowModeActivation = false;
    });
  };
  const hasWindowModeActivation = () => windowModeActivation;
  const consumeWindowModeActivation = () => {
    if (!windowModeActivation) return false;
    windowModeActivation = false;
    activationGeneration++;
    return true;
  };

  const fullscreenSupported = Boolean(__blitsenWindowMode("supported"));
  let pointerLockElement = null;
  let fullscreenElement = null;

  const modeError = (target, eventType, error) => {
    target.dispatchEvent(new Event(eventType, { bubbles: true }));
    return Promise.reject(error);
  };
  const requestPointerLock = (element, options) => {
    if (options === null || (typeof options !== "object" && typeof options !== "undefined"))
      return modeError(document, "pointerlockerror",
        new TypeError("pointer lock options must be an object"));
    if (!element.isConnected)
      return modeError(document, "pointerlockerror", new DOMException(
        "pointer lock requires an element in this document", "WrongDocumentError"));
    if (!hasWindowModeActivation())
      return modeError(document, "pointerlockerror", new DOMException(
        "pointer lock requires transient user activation", "NotAllowedError"));
    if (!fullscreenSupported)
      return modeError(document, "pointerlockerror", new DOMException(
        "pointer lock is not supported on this platform", "NotSupportedError"));
    try { __blitsenWindowMode("lockPointer"); }
    catch (error) {
      return modeError(document, "pointerlockerror", new DOMException(
        error?.message ?? "the platform refused pointer lock", "NotSupportedError"));
    }
    pointerLockElement = element;
    document.dispatchEvent(new Event("pointerlockchange"));
    return Promise.resolve();
  };
  const exitPointerLock = () => {
    if (pointerLockElement === null) return;
    try { __blitsenWindowMode("unlockPointer"); }
    finally {
      pointerLockElement = null;
      document.dispatchEvent(new Event("pointerlockchange"));
    }
  };

  const requestFullscreen = (element, options) => {
    if (options === null || (typeof options !== "object" && typeof options !== "undefined"))
      return modeError(element, "fullscreenerror", new TypeError("fullscreen options must be an object"));
    if (!element.isConnected)
      return modeError(element, "fullscreenerror",
        new TypeError("fullscreen requires an element in this document"));
    // Blitsen can make the native window fullscreen, but does not yet implement
    // the Fullscreen top layer needed to present an arbitrary subtree honestly.
    if (element !== document.documentElement)
      return modeError(element, "fullscreenerror", new DOMException(
        "Blitsen currently supports fullscreen only on document.documentElement", "NotSupportedError"));
    if (!consumeWindowModeActivation())
      return modeError(element, "fullscreenerror", new DOMException(
        "fullscreen requires transient user activation", "NotAllowedError"));
    if (!fullscreenSupported)
      return modeError(element, "fullscreenerror", new DOMException(
        "fullscreen is not supported on this platform", "NotSupportedError"));
    try { __blitsenWindowMode("enterFullscreen"); }
    catch (error) {
      return modeError(element, "fullscreenerror", new DOMException(
        error?.message ?? "the platform refused fullscreen", "NotSupportedError"));
    }
    fullscreenElement = element;
    element.dispatchEvent(new Event("fullscreenchange", { bubbles: true }));
    return Promise.resolve();
  };
  const exitFullscreen = () => {
    if (fullscreenElement === null) return Promise.resolve();
    const previous = fullscreenElement;
    try { __blitsenWindowMode("exitFullscreen"); }
    catch (error) {
      return modeError(previous, "fullscreenerror", new DOMException(
        error?.message ?? "the platform refused to leave fullscreen", "NotSupportedError"));
    }
    fullscreenElement = null;
    previous.dispatchEvent(new Event("fullscreenchange", { bubbles: true }));
    return Promise.resolve();
  };

  // Raw DeviceEvent deltas bypass hit testing and always reach the locked
  // element. The absolute coordinate pairs stay fixed until lock is released.
  const dispatchLockedPointerMotion = (movementX, movementY) => {
    const target = pointerLockElement;
    if (target === null || !target.isConnected) {
      if (target !== null) releaseWindowModes(true, false, "disconnected");
      return false;
    }
    return target.dispatchEvent(new MouseEvent("mousemove", {
      ...lastMousePosition,
      movementX: Number(movementX), movementY: Number(movementY),
      buttons: activePointers.get(MOUSE_POINTER_ID)?.buttons ?? 0,
      bubbles: true, cancelable: true, view: globalThis,
    }));
  };

  // Called after the native side has already restored the cursor/window. Focus
  // and surface loss are unconditional security releases, not requests an app
  // can cancel.
  const releaseWindowModes = (pointer, fullscreen, _reason) => {
    if (pointer && pointerLockElement !== null) {
      pointerLockElement = null;
      document.dispatchEvent(new Event("pointerlockchange"));
    }
    if (fullscreen && fullscreenElement !== null) {
      const previous = fullscreenElement;
      fullscreenElement = null;
      previous.dispatchEvent(new Event("fullscreenchange", { bubbles: true }));
    }
  };

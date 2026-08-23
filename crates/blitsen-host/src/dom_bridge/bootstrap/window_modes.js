  // One short-lived activation token, granted only by a host-dispatched native
  // pointer/key event. Production dispatch hooks are retained by Rust and are
  // not properties application scripts can call or replace.
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

  const pointerLockSupported = Boolean(hostWindowMode("pointerLockSupported"));
  const fullscreenSupported = Boolean(hostWindowMode("fullscreenSupported"));
  let pointerLockElement = null;
  let fullscreenElement = null;
  let pendingPointerLock = null;
  let pendingFullscreen = null;

  const queueModeTask = callback => hostSetTimeout(callback, 0);
  const modeError = (target, eventType, error) => new Promise((_, reject) => {
    queueModeTask(() => {
      target.dispatchEvent(new Event(eventType, { bubbles: true }));
      reject(error);
    });
  });

  // Pointer lock and pointer capture are mutually exclusive. Clear both the
  // active and requested override before publishing the lock, and report every
  // element whose capture was displaced before pointerlockchange.
  const releasePointerCapturesForLock = () => {
    const ids = new Set([...pointerCaptures.keys(), ...pendingPointerCaptures.keys()]);
    for (const pointerId of ids) {
      const active = pointerCaptures.get(pointerId) ?? null;
      const pending = pendingPointerCaptures.get(pointerId) ?? null;
      pointerCaptures.delete(pointerId);
      pendingPointerCaptures.delete(pointerId);
      const targets = active === pending ? [active] : [active, pending];
      for (const target of targets) if (target !== null) {
        target.dispatchEvent(new PointerEvent("lostpointercapture", {
          pointerId,
          pointerType: pointerId === MOUSE_POINTER_ID ? "mouse" : "",
          bubbles: true,
          cancelable: false,
        }));
      }
    }
  };

  const requestPointerLock = (element, options) => {
    if (options === null || (typeof options !== "object" && typeof options !== "undefined"))
      return modeError(document, "pointerlockerror",
        new TypeError("pointer lock options must be an object"));
    if (!element.isConnected)
      return modeError(document, "pointerlockerror", new DOMException(
        "pointer lock requires an element in this document", "WrongDocumentError"));
    if (options?.unadjustedMovement === true)
      return modeError(document, "pointerlockerror", new DOMException(
        "unadjustedMovement is not available from this platform input backend", "NotSupportedError"));
    if (pointerLockElement === element) return Promise.resolve();
    if (pendingPointerLock?.element === element) return pendingPointerLock.promise;
    if (!hasWindowModeActivation())
      return modeError(document, "pointerlockerror", new DOMException(
        "pointer lock requires transient user activation", "NotAllowedError"));
    if (!pointerLockSupported)
      return modeError(document, "pointerlockerror", new DOMException(
        "pointer lock is not supported on this platform", "NotSupportedError"));
    try { hostWindowMode("lockPointer"); }
    catch (error) {
      return modeError(document, "pointerlockerror", new DOMException(
        error?.message ?? "the platform refused pointer lock", "NotSupportedError"));
    }
    let resolveRequest;
    let rejectRequest;
    const promise = new Promise((resolve, reject) => {
      resolveRequest = resolve;
      rejectRequest = reject;
    });
    const request = { element, promise, resolve: resolveRequest, reject: rejectRequest };
    pendingPointerLock = request;
    queueModeTask(() => {
      if (pendingPointerLock !== request) return;
      pendingPointerLock = null;
      if (!element.isConnected) {
        try { hostWindowMode("unlockPointer"); } catch {}
        document.dispatchEvent(new Event("pointerlockerror"));
        rejectRequest(new DOMException("pointer lock target was disconnected", "WrongDocumentError"));
        return;
      }
      releasePointerCapturesForLock();
      const changed = pointerLockElement !== element;
      pointerLockElement = element;
      if (changed) document.dispatchEvent(new Event("pointerlockchange"));
      resolveRequest();
    });
    return promise;
  };

  const releasePointerLock = (nativeAlreadyReleased, reason) => {
    const pending = pendingPointerLock;
    const previous = pointerLockElement;
    if (pending === null && previous === null) return false;
    if (!nativeAlreadyReleased) try { hostWindowMode("unlockPointer"); } catch {}
    pendingPointerLock = null;
    pointerLockElement = null;
    queueModeTask(() => {
      if (previous !== null) document.dispatchEvent(new Event("pointerlockchange"));
      if (pending !== null) {
        document.dispatchEvent(new Event("pointerlockerror"));
        pending.reject(new DOMException(`pointer lock ended before acquisition (${reason})`, "AbortError"));
      }
    });
    return true;
  };

  const exitPointerLock = () => { releasePointerLock(false, "explicit-exit"); };

  const requestFullscreen = (element, options) => {
    if (options === null || (typeof options !== "object" && typeof options !== "undefined"))
      return modeError(element, "fullscreenerror", new TypeError("fullscreen options must be an object"));
    if (!element.isConnected)
      return modeError(element, "fullscreenerror",
        new TypeError("fullscreen requires an element in this document"));
    if (element !== document.documentElement)
      return modeError(element, "fullscreenerror", new DOMException(
        "Blitsen currently supports fullscreen only on document.documentElement", "NotSupportedError"));
    if (fullscreenElement === element) return Promise.resolve();
    if (pendingFullscreen?.element === element) return pendingFullscreen.promise;
    if (!consumeWindowModeActivation())
      return modeError(element, "fullscreenerror", new DOMException(
        "fullscreen requires transient user activation", "NotAllowedError"));
    if (!fullscreenSupported)
      return modeError(element, "fullscreenerror", new DOMException(
        "fullscreen is not supported on this platform", "NotSupportedError"));
    try { hostWindowMode("enterFullscreen"); }
    catch (error) {
      return modeError(element, "fullscreenerror", new DOMException(
        error?.message ?? "the platform refused fullscreen", "NotSupportedError"));
    }
    let resolveRequest;
    let rejectRequest;
    const promise = new Promise((resolve, reject) => {
      resolveRequest = resolve;
      rejectRequest = reject;
    });
    const request = { element, promise, resolve: resolveRequest, reject: rejectRequest };
    pendingFullscreen = request;
    queueModeTask(() => {
      if (pendingFullscreen !== request) return;
      pendingFullscreen = null;
      if (!element.isConnected) {
        try { hostWindowMode("exitFullscreen"); } catch {}
        element.dispatchEvent(new Event("fullscreenerror", { bubbles: true }));
        rejectRequest(new TypeError("fullscreen target was disconnected"));
        return;
      }
      const changed = fullscreenElement !== element;
      fullscreenElement = element;
      if (changed) element.dispatchEvent(new Event("fullscreenchange", { bubbles: true }));
      resolveRequest();
    });
    return promise;
  };

  const releaseFullscreen = (nativeAlreadyReleased, reason) => {
    const pending = pendingFullscreen;
    const previous = fullscreenElement;
    if (pending === null && previous === null) return Promise.resolve();
    if (!nativeAlreadyReleased) try { hostWindowMode("exitFullscreen"); }
    catch (error) {
      return modeError(previous ?? document, "fullscreenerror", new DOMException(
        error?.message ?? "the platform refused to leave fullscreen", "NotSupportedError"));
    }
    pendingFullscreen = null;
    fullscreenElement = null;
    return new Promise(resolve => queueModeTask(() => {
      if (previous !== null)
        previous.dispatchEvent(new Event("fullscreenchange", { bubbles: true }));
      if (pending !== null) {
        pending.element.dispatchEvent(new Event("fullscreenerror", { bubbles: true }));
        pending.reject(new DOMException(`fullscreen ended before acquisition (${reason})`, "AbortError"));
      }
      resolve();
    }));
  };

  const exitFullscreen = () => releaseFullscreen(false, "explicit-exit");

  const dispatchLockedPointerMotion = (movementX, movementY) => {
    const target = pointerLockElement;
    if (target === null || !target.isConnected) {
      if (target !== null) releasePointerLock(false, "disconnected");
      return false;
    }
    return target.dispatchEvent(new MouseEvent("mousemove", {
      ...lastMousePosition,
      movementX: Number(movementX), movementY: Number(movementY),
      buttons: activePointers.get(MOUSE_POINTER_ID)?.buttons ?? 0,
      bubbles: true, cancelable: true, view: globalThis,
    }));
  };

  const releaseWindowModes = (pointer, fullscreen, reason) => {
    if (pointer) releasePointerLock(true, String(reason));
    if (fullscreen) void releaseFullscreen(true, String(reason));
    return true;
  };

  windowModesTreeMutation = () => {
    const pointer = pendingPointerLock?.element ?? pointerLockElement;
    if (pointer !== null && !pointer.isConnected)
      releasePointerLock(false, "target-disconnected");
    const fullscreen = pendingFullscreen?.element ?? fullscreenElement;
    if (fullscreen !== null && !fullscreen.isConnected)
      void releaseFullscreen(false, "target-disconnected");
  };

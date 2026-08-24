  const gamepadInstalled = typeof globalThis.__blitsenGamepads === "function";
  const gamepadToken = Symbol("Blitsen gamepad snapshot");

  class GamepadButton {
    constructor(token, raw) {
      if (token !== gamepadToken) throw new TypeError("Illegal constructor");
      defineMembers(this, {
        pressed: Boolean(raw.pressed),
        touched: Boolean(raw.touched),
        value: Number(raw.value),
      });
      Object.freeze(this);
    }
  }

  const gamepadCommands = new Map();
  const gamepadDeviceListeners = new Set();
  const gamepadPending = gamepadInstalled ? __blitsenGamepadPending : () => false;
  const gamepadWorkPending = () => gamepadCommands.size > 0 || gamepadPending();
  const startGamepadVibration = (index, strong, weak, duration, startDelay = 0) => {
    const id = __blitsenGamepadVibrate(
      String(index), String(strong), String(weak), String(duration), String(startDelay));
    return new Promise((resolve, reject) => gamepadCommands.set(String(id), { resolve, reject }));
  };

  class GamepadHapticActuator {
    constructor(token, index) {
      if (token !== gamepadToken) throw new TypeError("Illegal constructor");
      defineMembers(this, { type: "dual-rumble", effects: Object.freeze(["dual-rumble"]) });
      Object.defineProperty(this, "_index", { value: Number(index) });
      Object.freeze(this);
    }
    playEffect(type, parameters = {}) {
      if (String(type) !== "dual-rumble")
        return Promise.reject(new DOMException(`unsupported gamepad effect: ${type}`, "NotSupportedError"));
      const duration = Number(parameters.duration ?? 0);
      const startDelay = Number(parameters.startDelay ?? 0);
      const strong = Number(parameters.strongMagnitude ?? 0);
      const weak = Number(parameters.weakMagnitude ?? 0);
      if (![duration, startDelay, strong, weak].every(Number.isFinite)
        || duration < 0 || duration > 60_000 || startDelay < 0 || startDelay > 60_000
        || strong < 0 || strong > 1 || weak < 0 || weak > 1)
        return Promise.reject(new TypeError(
          "gamepad duration/startDelay must be 0..60000ms and magnitudes must be 0..1"));
      return startGamepadVibration(this._index, strong, weak, duration, startDelay);
    }
    async reset() {
      return startGamepadVibration(this._index, 0, 0, 0);
    }
  }

  class Gamepad {
    constructor(token, raw) {
      if (token !== gamepadToken) throw new TypeError("Illegal constructor");
      const index = Number(raw.index);
      defineMembers(this, {
        id: String(raw.id), index, connected: Boolean(raw.connected),
        timestamp: Number(raw.timestamp), mapping: String(raw.mapping),
        axes: Object.freeze(raw.axes.map(Number)),
        buttons: Object.freeze(raw.buttons.map(button => new GamepadButton(gamepadToken, button))),
        vibrationActuator: raw.vibrationActuator
          ? new GamepadHapticActuator(gamepadToken, index) : null,
      });
      Object.freeze(this);
    }
  }

  class GamepadEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      defineMembers(this, { gamepad: options.gamepad ?? null });
    }
  }

  const gamepadFromRaw = raw => raw === null ? null : new Gamepad(gamepadToken, raw);
  const gamepadSnapshots = () => {
    if (!gamepadInstalled) return Object.freeze([]);
    return Object.freeze(JSON.parse(__blitsenGamepads()).map(gamepadFromRaw));
  };
  const gamepadListener = listener => {
    if (typeof listener !== "function")
      throw new TypeError("gamepad device-change listener must be a function");
    gamepadDeviceListeners.add(listener);
    return () => { gamepadDeviceListeners.delete(listener); };
  };
  const settleGamepads = () => {
    if (!gamepadInstalled || !gamepadPending()) return;
    for (const message of JSON.parse(__blitsenGamepadTake())) {
      if (message.type === "completion") {
        const command = gamepadCommands.get(String(message.commandId));
        if (!command) continue;
        gamepadCommands.delete(String(message.commandId));
        if (message.error === null) command.resolve(message.result);
        else command.reject(new DOMException(message.error, message.errorName ?? "OperationError"));
        continue;
      }
      const gamepad = gamepadFromRaw(message.gamepad);
      globalThis.dispatchEvent(new GamepadEvent(`gamepad${message.kind}`, { gamepad }));
      const nativeEvent = Object.freeze({
        type: message.kind, index: gamepad.index, id: gamepad.id,
      });
      for (const listener of gamepadDeviceListeners) {
        try { listener(nativeEvent); }
        catch (error) { console.error("Uncaught exception in gamepad device-change listener", error); }
      }
    }
  };

  // Android and unrecognised targets compile no controller backend. Keep the
  // member genuinely absent there so feature detection does not mistake an
  // always-empty array for controller support.
  if (!gamepadInstalled) try { delete Navigator.prototype.getGamepads; } catch {}

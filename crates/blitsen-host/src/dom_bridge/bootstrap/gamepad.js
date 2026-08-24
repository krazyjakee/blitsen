  const gamepadInstalled = typeof globalThis.__blitsenGamepads === "function";
  const gamepadToken = Symbol("Blitsen gamepad snapshot");
  let gamepadTouched = false;
  const touchGamepads = () => {
    if (!gamepadInstalled || gamepadTouched) return;
    __blitsenGamepadTouch();
    gamepadTouched = true;
  };

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

  const gamepadDeviceListeners = new Set();
  const gamepadPending = gamepadInstalled ? __blitsenGamepadPending : () => false;
  const gamepadChannel = makeCommandChannel({
    pending: gamepadPending,
    take: () => JSON.parse(__blitsenGamepadTake()),
    result: message => message.result,
    error: message => new DOMException(message.error, message.errorName ?? "OperationError"),
    onMessage: message => {
      const gamepad = gamepadFromRaw(message.gamepad);
      globalThis.dispatchEvent(new GamepadEvent(`gamepad${message.kind}`, { gamepad }));
      deliverCommandListeners(gamepadDeviceListeners, Object.freeze({
        type: message.kind, index: gamepad.index, id: gamepad.id,
      }), "gamepad device-change");
    },
  });
  const gamepadWorkPending = gamepadChannel.workPending;
  const startGamepadVibration = (index, strong, weak, duration, startDelay = 0) => {
    touchGamepads();
    const id = __blitsenGamepadVibrate(
      String(index), String(strong), String(weak), String(duration), String(startDelay));
    return gamepadChannel.run(id);
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
    touchGamepads();
    return Object.freeze(JSON.parse(__blitsenGamepads()).map(gamepadFromRaw));
  };
  const gamepadListener = listener => {
    if (typeof listener !== "function")
      throw new TypeError("gamepad device-change listener must be a function");
    touchGamepads();
    gamepadDeviceListeners.add(listener);
    return () => { gamepadDeviceListeners.delete(listener); };
  };
  const settleGamepads = gamepadChannel.settle;

  // Android and unrecognised targets compile no controller backend. Keep the
  // member genuinely absent there so feature detection does not mistake an
  // always-empty array for controller support.
  if (!gamepadInstalled) try { delete Navigator.prototype.getGamepads; } catch {}

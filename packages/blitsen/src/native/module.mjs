// Backs every `blitsen/<module>` subpath.
//
// The design constraint is that two different failures must stay distinguishable.
// Importing `blitsen/dialog` in a browser or a plain Node script is a mistake, and
// should say so loudly. A capability this runtime version does not implement yet is
// not a mistake — it must read as absent, so `if (dialog.openFile)` works. Principle
// 4: partial is fine, incoherent is not.
//
// So: inside the runtime the namespace exposes exactly what the host installed, and
// anything missing is genuinely `undefined`. Outside it, every access throws.

const RUNTIME = Symbol.for("blitsen.native");

export function nativeModule(name) {
  return new Proxy(Object.create(null), {
    get(_target, property) {
      // Let the module answer the questions a bundler or a `typeof` check asks
      // without pretending to be inside the runtime.
      if (property === Symbol.toStringTag) return `BlitsenNative(${name})`;
      if (property === "then") return undefined;

      const runtime = globalThis[RUNTIME];
      if (!runtime) {
        throw new Error(`blitsen/${name} requires the Blitsen runtime: `
          + `"${String(property)}" was accessed in a plain JavaScript host. `
          + "Native modules exist only inside an application launched by blitsen.");
      }
      // Absent capability, not an error: feature detection has to work.
      return runtime[name]?.[property];
    },
    has(_target, property) {
      const runtime = globalThis[RUNTIME];
      return runtime ? property in (runtime[name] ?? {}) : false;
    },
    ownKeys() {
      const runtime = globalThis[RUNTIME];
      return runtime ? Reflect.ownKeys(runtime[name] ?? {}) : [];
    },
    getOwnPropertyDescriptor(_target, property) {
      const runtime = globalThis[RUNTIME];
      const value = runtime?.[name]?.[property];
      if (value === undefined) return undefined;
      return { value, enumerable: true, configurable: true, writable: false };
    },
    set() { throw new Error(`blitsen/${name} is read-only`); },
  });
}

/// Modules the specifier layer knows about. A name here does not imply the runtime
/// implements it — that is exactly what the namespace reports at run time.
export const NATIVE_MODULES = [
  "app", "window", "dialog", "clipboard", "tray", "notify", "input", "hid", "os",
];

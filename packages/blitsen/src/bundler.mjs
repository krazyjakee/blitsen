// Compatibility helpers for configurations that still mention `native:*`.
//
// The shipped module loader has no builtin-module namespace: leaving one of these
// specifiers external only postpones the failure until application startup. Refuse it
// in the bundler, where the replacement is attributable, and direct the application at
// the real package subpath every bundler can include.
import { NATIVE_MODULES } from "./native/module.mjs";

const PREFIX = "native:";
const isNative = specifier =>
  specifier.startsWith(PREFIX) && NATIVE_MODULES.includes(specifier.slice(PREFIX.length));

const unknown = specifier => {
  const name = specifier.slice(PREFIX.length);
  return `unknown native module "${specifier}" (known modules: `
    + `${NATIVE_MODULES.map(module => `${PREFIX}${module}`).join(", ")})`
    + (name.includes("/") ? "; native modules have no subpaths" : "");
};

const unsupported = specifier => `"${specifier}" is not a Blitsen runtime module; import `
  + `"blitsen/${specifier.slice(PREFIX.length)}" instead. Leaving native:* external produces `
  + "an unresolved bare import in the shipped application.";

const errorFor = specifier => isNative(specifier) ? unsupported(specifier) : unknown(specifier);

/// Vite, Rollup and any bundler taking a Rollup-shaped plugin.
export function blitsenRollup() {
  return {
    name: "blitsen-native",
    // `enforce: "pre"` so Vite's own resolver does not reject the specifier first.
    enforce: "pre",
    resolveId(specifier) {
      if (specifier.startsWith(PREFIX)) this.error(errorFor(specifier));
      return null;
    },
  };
}

export const blitsenVite = blitsenRollup;

/// esbuild.
export function blitsenEsbuild() {
  return {
    name: "blitsen-native",
    setup(build) {
      build.onResolve({ filter: /^native:/ }, ({ path }) => {
        return { errors: [{ text: errorFor(path) }] };
      });
    },
  };
}

/// webpack. Returns the value for `externals`, not a plugin — webpack models this as
/// an externals function, and a function composes with a user's existing entry.
export function blitsenWebpackExternals() {
  return ({ request }, callback) => {
    if (request && request.startsWith(PREFIX)) return callback(new Error(errorFor(request)));
    return callback();
  };
}

export { NATIVE_MODULES };

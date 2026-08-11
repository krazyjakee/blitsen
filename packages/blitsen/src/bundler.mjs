// Optional plugins for users who prefer the `native:*` spelling.
//
// `blitsen/*` is the recommended form precisely because it needs none of this: it is
// an ordinary package subpath that every bundler already resolves. A bare `native:`
// specifier is not resolvable, so a bundler either tries to bundle it and fails, or
// errors outright. These plugins mark it external and leave it for the runtime.
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

/// Vite, Rollup and any bundler taking a Rollup-shaped plugin.
export function blitsenRollup() {
  return {
    name: "blitsen-native",
    // `enforce: "pre"` so Vite's own resolver does not reject the specifier first.
    enforce: "pre",
    resolveId(specifier) {
      if (isNative(specifier)) return { id: specifier, external: true };
      if (specifier.startsWith(PREFIX)) this.error(unknown(specifier));
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
        if (isNative(path)) return { path, external: true };
        return { errors: [{ text: unknown(path) }] };
      });
    },
  };
}

/// webpack. Returns the value for `externals`, not a plugin — webpack models this as
/// an externals function, and a function composes with a user's existing entry.
export function blitsenWebpackExternals() {
  return ({ request }, callback) => {
    if (request && isNative(request)) return callback(null, `module ${request}`);
    if (request && request.startsWith(PREFIX)) return callback(new Error(unknown(request)));
    return callback();
  };
}

export { NATIVE_MODULES };

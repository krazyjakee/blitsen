/** Native module names the specifier layer knows about. */
export declare const NATIVE_MODULES: readonly string[];

/** Marks `native:*` external. Works with Rollup and Vite. */
export declare function blitsenRollup(): {
  name: string;
  enforce: "pre";
  resolveId(specifier: string): { id: string; external: true } | null;
};

/** Alias of {@link blitsenRollup}, for readability in a Vite config. */
export declare const blitsenVite: typeof blitsenRollup;

/** Marks `native:*` external in esbuild. */
export declare function blitsenEsbuild(): {
  name: string;
  setup(build: unknown): void;
};

/**
 * Value for webpack's `externals`. A function rather than a plugin, because that is
 * how webpack models this and it composes with an existing `externals` entry.
 */
export declare function blitsenWebpackExternals(): (
  data: { request?: string },
  callback: (error?: Error | null, result?: string) => void,
) => void;

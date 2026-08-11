/**
 * A native module namespace.
 *
 * Members are whatever the running Blitsen version installed. A capability this
 * version does not implement is `undefined`, so feature detection works:
 *
 * ```js
 * import dialog from "blitsen/dialog";
 * if (dialog.openFile) { … }
 * ```
 *
 * Outside the Blitsen runtime — a browser, a plain Node script — every access
 * throws, because that is a mistake rather than a missing capability.
 *
 * Each module gains typed members with its own implementation. Until then the
 * namespace is deliberately untyped rather than declaring an API that does not
 * exist yet.
 */
declare const nativeModule: Record<string, unknown>;
export default nativeModule;

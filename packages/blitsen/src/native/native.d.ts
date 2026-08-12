/**
 * A second invocation of the application, handed to the instance holding the
 * single-instance lock.
 */
export interface Invocation {
  /** The second process's command line, exactly as the OS gave it. */
  readonly argv: readonly string[];
  /** Its working directory, so a relative path in `argv` still resolves. */
  readonly cwd: string;
}

/** Clipboard pixels: 8-bit RGBA, row-major, `width * height * 4` bytes. */
export interface ClipboardImage {
  /** Width in pixels. */
  readonly width: number;
  /** Height in pixels. */
  readonly height: number;
  /** The pixels, as read from the clipboard. */
  readonly data: Uint8Array;
}

/** `blitsen/app`: what the application is, rather than what it is showing. */
export interface NativeApp {
  /** Platform directory for state that must survive a restart, not created. */
  dataDir?(name: string): string;
  /** Platform directory for state the system may delete at any time. */
  cacheDir?(name: string): string;
  /** Platform directory for user-editable configuration. */
  configDir?(name: string): string;
  /**
   * Claims the single-instance lock, returning `false` when another instance
   * already holds it — in which case this invocation was handed to that
   * instance and this process should `process.exit(0)`.
   *
   * Unix only.
   */
  requestSingleInstanceLock?(
    name: string,
    onSecondInstance?: ((invocation: Invocation) => void) | null,
  ): boolean;
  /**
   * Spawns a copy of this process with the same arguments and releases the
   * single-instance lock. Stopping this one is `process.exit`.
   */
  relaunch?(): void;
}

/** `blitsen/clipboard`: the system clipboard, in the flavours it carries. */
export interface NativeClipboard {
  /** The clipboard as plain text, or `null` when it holds no text. */
  readText?(): string | null;
  /** The clipboard's HTML flavour, or `null` when it has none. */
  readHtml?(): string | null;
  /** The clipboard as an image, or `null` when it holds none this can decode. */
  readImage?(): ClipboardImage | null;
  /** Replaces the clipboard contents with plain text. */
  writeText?(text: string): void;
  /** Replaces them with HTML, plus the text a plain-text paste receives. */
  writeHtml?(html: string, alternative?: string): void;
  /** Replaces them with an image. */
  writeImage?(image: {
    width: number;
    height: number;
    data: Uint8Array | Uint8ClampedArray;
  }): void;
  /** Empties the clipboard. */
  clear?(): void;
}

/**
 * A native module namespace.
 *
 * Members are whatever the running Blitsen version installed. A capability this
 * version does not implement is `undefined` — which is why every member above is
 * optional — so feature detection works:
 *
 * ```js
 * import clipboard from "blitsen/clipboard";
 * if (clipboard.readImage) { … }
 * ```
 *
 * Outside the Blitsen runtime — a browser, a plain Node script — every access
 * throws, because that is a mistake rather than a missing capability.
 *
 * One declaration file backs every `blitsen/<module>` subpath, so the members of
 * the modules that have been implemented are declared together here. The index
 * signature is what the rest are: a module gains its own members with its own
 * implementation, rather than declaring an API that does not exist yet.
 */
declare const nativeModule: NativeApp & NativeClipboard & Record<string, unknown>;
export default nativeModule;

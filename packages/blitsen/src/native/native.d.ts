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

/** One monitor, as the window's own display server describes it. */
export interface Monitor {
  /** The name the desktop gives it, where it has one. */
  readonly name: string | null;
  /** Left edge in desktop coordinates, in physical pixels. */
  readonly x: number | null;
  /** Top edge in desktop coordinates, in physical pixels. */
  readonly y: number | null;
  /** Width of the current video mode, in physical pixels. */
  readonly width: number | null;
  /** Height of the current video mode, in physical pixels. */
  readonly height: number | null;
  /** Physical pixels per CSS pixel on this monitor, which may differ per monitor. */
  readonly scaleFactor: number;
  /** Refresh rate in Hz, where the display server reports one. */
  readonly refreshRate: number | null;
  /** Whether the application window is on this monitor. */
  readonly current: boolean;
  /** Whether the desktop calls this the primary monitor. */
  readonly primary: boolean;
}

/** How far the cursor is held to the window. */
export type CursorGrab = "none" | "confined" | "locked";

/** A named group of extensions a file dialog offers. */
export interface DialogFilter {
  /** What the group is called in the dialog's filter list. */
  name: string;
  /** Extensions without their leading dot. */
  extensions: readonly string[];
}

/** What a file dialog is asked for. */
export interface FileDialogOptions {
  /** Dialog title, or the platform's own wording. */
  title?: string;
  /** Directory to open in. */
  directory?: string;
  /** File name to suggest, for `saveFile`. */
  fileName?: string;
  /** Extension groups to offer, in order. */
  filters?: readonly DialogFilter[];
}

/** What a message dialog is asked for. */
export interface MessageDialogOptions {
  /** Dialog title. */
  title?: string;
  /** Body text. */
  message?: string;
  /** How urgent it is, which the platform draws as an icon. */
  level?: "info" | "warning" | "error";
  /** Which buttons to offer. */
  buttons?: "ok" | "okCancel" | "yesNo" | "yesNoCancel";
}

/**
 * `blitsen/window`: the window this run opened.
 *
 * Its size and pixel density are not here — `innerWidth`, `innerHeight`,
 * `devicePixelRatio` and the `resize` event already answer those, and a second
 * answer that could disagree would be worse than none. What is new is the
 * commands, and the monitors including the ones the window is not on.
 *
 * Every member needs the window, which exists from the `load` event onwards; a
 * call from a document script running before then throws saying so.
 */
export interface NativeWindow {
  /** Asks the window manager for a new size, in CSS pixels. */
  setSize?(width: number, height: number): void;
  /** Enters or leaves borderless fullscreen on the window's current monitor. */
  setFullscreen?(fullscreen: boolean): void;
  /** Whether the window is fullscreen. */
  isFullscreen?(): boolean;
  /** Shows or hides the title bar and border. */
  setDecorations?(decorations: boolean): void;
  /** Whether the window has a title bar and border. */
  isDecorated?(): boolean;
  /** Keeps the window above others. Wayland has no protocol for this and ignores it. */
  setAlwaysOnTop?(alwaysOnTop: boolean): void;
  /** Sets the cursor to a CSS cursor keyword, such as `"pointer"` or `"grabbing"`. */
  setCursor?(cursor: string): void;
  /** Shows or hides the cursor over the window. */
  setCursorVisible?(visible: boolean): void;
  /** Confines or locks the cursor; throws where the platform cannot do it. */
  setCursorGrab?(mode: CursorGrab): void;
  /** Every monitor the desktop offers, each with its own scale factor. */
  monitors?(): Monitor[];
}

/**
 * `blitsen/dialog`: the desktop's own file and message dialogs.
 *
 * Each returns a promise and the frame loop keeps turning while the dialog is
 * open, so `requestAnimationFrame` keeps firing and the window keeps painting
 * behind it. The dialog is modal to the application window regardless, because
 * the desktop draws it: it needs that window, so these are usable from the
 * `load` event onwards.
 *
 * A file dialog answers real filesystem paths, and `null` when it was dismissed.
 */
export interface NativeDialog {
  /** Chooses one existing file. */
  openFile?(options?: FileDialogOptions): Promise<string | null>;
  /** Chooses any number of existing files. */
  openFiles?(options?: FileDialogOptions): Promise<string[] | null>;
  /** Chooses a path to write to, which need not exist yet. */
  saveFile?(options?: FileDialogOptions): Promise<string | null>;
  /** Chooses one existing directory. */
  openFolder?(options?: FileDialogOptions): Promise<string | null>;
  /** Chooses any number of existing directories. */
  openFolders?(options?: FileDialogOptions): Promise<string[] | null>;
  /** Shows a message and resolves to the button it was dismissed with. */
  message?(options?: MessageDialogOptions): Promise<"ok" | "cancel" | "yes" | "no">;
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
 * A `native:` module that installs nothing in this version.
 *
 * Not an error and not an empty object: the subpath resolves, and every member
 * reads `undefined` inside the runtime, so `if (tray.create)` is the same
 * feature detection it is on a module that does have members. What it must not
 * do is name methods — a declared `create` that no runtime installs is exactly
 * the drift these definitions are checked against.
 */
export interface NativeUnimplemented {
  readonly [member: string]: undefined;
}

/**
 * What a native module namespace is, whichever module it is.
 *
 * Members are whatever the running Blitsen version installed. A capability this
 * version does not implement is `undefined` — which is why every member of every
 * interface above is optional — so feature detection works:
 *
 * ```ts
 * import clipboard from "blitsen/clipboard";
 * if (clipboard.readImage) { … }
 * ```
 *
 * Outside the Blitsen runtime — a browser, a plain Node script — every access
 * throws, because that is a mistake rather than a missing capability. The index
 * signature is what an unlisted member is: `unknown`, so it must be narrowed
 * before it can be called, rather than `any`.
 *
 * Each `blitsen/<module>` subpath has its own declaration file naming its own
 * interface, so importing `blitsen/app` does not offer the clipboard's methods.
 */
export type NativeNamespace<Members> = Members & { readonly [member: string]: unknown };

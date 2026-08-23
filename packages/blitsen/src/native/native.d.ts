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
  /** Minimizes or restores the window. */
  setMinimized?(minimized: boolean): void;
  /** Maximizes or restores the window. */
  setMaximized?(maximized: boolean): void;
  /** Whether the window is currently maximized. */
  isMaximized?(): boolean;
  /** Hands an application-drawn title-bar press to the system window mover. */
  startDrag?(): void;
  /** Closes the application window. */
  close?(): void;
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

/** A built-in tray action handled without entering application JavaScript. */
export type TrayBuiltinAction = "show" | "hide" | "quit";

interface TrayActionMenuItemBase {
  /** Explicit discriminator; omitted for compatibility with the original flat menu shape. */
  type?: "action";
  /** User-facing menu label. */
  label: string;
  /** Whether the item can be activated. Defaults to true. */
  enabled?: boolean;
  /** Native keyboard accelerator such as `CmdOrCtrl+Shift+KeyP`. */
  accelerator?: string;
  /** PNG file contents displayed beside this action where native menus support icons. */
  icon?: Uint8Array | Uint8ClampedArray;
}

/** A tray entry whose activation is delivered to `onAction`. */
export interface TrayEventMenuItem extends TrayActionMenuItemBase {
  /** Stable application-defined identifier delivered with the action event. */
  id: string;
  action?: never;
}

/** A tray entry handled directly by the native window session. */
export interface TrayBuiltinMenuItem extends Omit<TrayActionMenuItemBase, "label"> {
  action: TrayBuiltinAction;
  id?: never;
  /** Override for the platform-neutral default label. */
  label?: string;
}

/** A visual separator. The action spelling preserves compatibility with package configuration. */
export type TraySeparatorMenuItem =
  | { type: "separator"; action?: never }
  | { action: "separator"; type?: never };

/** A checkable action with state reported in the action event. */
export interface TrayCheckboxMenuItem {
  type: "checkbox";
  id: string;
  label: string;
  enabled?: boolean;
  /** Initial state. Defaults to false. */
  checked?: boolean;
  accelerator?: string;
  action?: never;
  group?: never;
}

/** One choice in a consecutive radio group; exactly one item per group must be checked. */
export interface TrayRadioMenuItem {
  type: "radio";
  id: string;
  label: string;
  group: string;
  enabled?: boolean;
  checked?: boolean;
  accelerator?: string;
  action?: never;
}

/** A nested menu. IDs remain unique across the complete tree. */
export interface TraySubmenuItem {
  type: "submenu";
  label: string;
  enabled?: boolean;
  icon?: Uint8Array | Uint8ClampedArray;
  menu: readonly TrayMenuItem[];
  id?: never;
  action?: never;
}

export type TrayMenuItem =
  | TrayEventMenuItem
  | TrayBuiltinMenuItem
  | TraySeparatorMenuItem
  | TrayCheckboxMenuItem
  | TrayRadioMenuItem
  | TraySubmenuItem;

/** Runtime state for the application's single system tray icon. */
export interface RuntimeTrayOptions {
  /** PNG file contents. Paths are intentionally not resolved relative to an ambiguous caller. */
  icon: Uint8Array | Uint8ClampedArray;
  tooltip?: string | null;
  /** Show and focus the window on a primary click. Defaults to true. */
  openOnClick?: boolean;
  /** Hide the window when its close control is used. Defaults to false. */
  closeToTray?: boolean;
  menu?: readonly TrayMenuItem[];
}

export interface TrayClickEvent {
  readonly type: "click";
}

export interface TrayActionEvent {
  readonly type: "action";
  readonly id: string;
  /** New state for checkbox/radio actions; absent for ordinary actions. */
  readonly checked?: boolean;
}

/** `blitsen/tray`: the tray owned by this native window session. */
export interface NativeTray {
  /** Creates or atomically replaces the tray, including one created by package configuration. */
  configure?(options: RuntimeTrayOptions): Promise<void>;
  /** Removes the current tray. */
  remove?(): Promise<void>;
  /** Listens for primary activation of the tray icon; returns an unsubscribe function. */
  onClick?(listener: (event: TrayClickEvent) => void): () => void;
  /** Listens for activation of an application-defined menu item. */
  onAction?(listener: (event: TrayActionEvent) => void): () => void;
}

/** One currently held physical key. */
export interface PressedKey {
  /** Layout-independent DOM physical code, such as `KeyA` or `ArrowLeft`. */
  readonly code: string;
  /** Layout-dependent key value observed when the key was pressed. */
  readonly key: string;
}

/** Pointer state at the instant an input snapshot was taken. */
export interface NativePointerState {
  /** Position in CSS pixels, or null before the pointer has entered the window. */
  readonly x: number | null;
  readonly y: number | null;
  /** Held buttons: primary, secondary, auxiliary, back, forward or other-N. */
  readonly buttons: readonly string[];
  /** Raw device movement accumulated since the previous snapshot. */
  readonly movementX: number;
  readonly movementY: number;
  /** Wheel deltas accumulated since the previous snapshot, preserving their units. */
  readonly wheelLineX: number;
  readonly wheelLineY: number;
  readonly wheelPixelX: number;
  readonly wheelPixelY: number;
}

/** An atomic reading of the focus-scoped native input state. */
export interface NativeInputSnapshot {
  /** Increases whenever input state changes. */
  readonly sequence: number;
  readonly focused: boolean;
  readonly keys: readonly PressedKey[];
  readonly pointer: NativePointerState;
}

/** `blitsen/input`: polling state that complements ordinary DOM input events. */
export interface NativeInput {
  /** Reads held state and consumes accumulated movement and wheel deltas. */
  snapshot?(): NativeInputSnapshot;
}

export interface NativeNotificationOptions {
  /** Required title shown by the platform notification centre. */
  title: string;
  body?: string;
  /** Grouping/application name where the platform accepts one. */
  appName?: string;
  /** Milliseconds before expiry; zero requests a persistent notification. */
  timeout?: number;
  urgency?: "low" | "normal" | "critical";
  /** Icon name or absolute image path. Rejected on macOS, whose centre uses the app icon. */
  icon?: string;
  /** Buttons whose identifiers are returned by `onEvent`. */
  actions?: readonly NativeNotificationAction[];
}

export interface NativeNotificationAction {
  /** Stable application-defined identifier; `"default"` is reserved for body clicks. */
  id: string;
  title: string;
}

/** Fields that can replace an active notification; omitted fields are preserved. */
export type NativeNotificationUpdate = Partial<NativeNotificationOptions>;

export type NativeNotificationPermission = "default" | "denied" | "granted";

export type NativeNotificationEvent =
  | Readonly<{ type: "show"; id: string }>
  | Readonly<{ type: "click"; id: string }>
  | Readonly<{ type: "action"; id: string; action: string }>
  | Readonly<{
      type: "close";
      id: string;
      reason: "expired" | "dismissed" | "closed" | "unknown";
    }>
  | Readonly<{ type: "error"; id: string; message: string }>;

/** `blitsen/notify`: native notification capabilities beyond the web surface. */
export interface NativeNotify {
  /** Submits a notification and returns an identifier valid for this application session. */
  show?(options: NativeNotificationOptions): Promise<string>;
  /** Reads the platform authorization state without prompting. */
  permission?(): Promise<NativeNotificationPermission>;
  /** Requests authorization where the platform has a prompt, then returns its state. */
  requestPermission?(): Promise<NativeNotificationPermission>;
  /** Replaces fields on an active notification; false means the ID is no longer active. */
  update?(id: string, options: NativeNotificationUpdate): Promise<boolean>;
  /** Closes an active notification; false means the ID is unknown or no longer active. */
  close?(id: string): Promise<boolean>;
  /** Subscribes to FIFO lifecycle events and returns an unsubscribe function. */
  onEvent?(listener: (event: NativeNotificationEvent) => void): () => void;
}

/** One logical processor: a hardware thread as the OS schedules onto it. */
export interface CpuCore {
  /** What the OS calls it — `cpu0`, `CPU 0`. */
  readonly name: string;
  /** Current clock in MHz, or 0 where the platform reports none. */
  readonly frequency: number;
  /** Share of this core busy since the previous `cpu()` call, 0–100. */
  readonly usage: number;
}

/** The processor: a spec sheet, plus a sample taken when `cpu()` was called. */
export interface Cpu {
  /**
   * Marketing name, such as `"AMD Ryzen 9 5900X 12-Core Processor"`, or `null`
   * where the platform does not carry one. Reported on Linux, macOS and x64
   * Windows; arm64 Windows has no processor name in the registry to read.
   */
  readonly brand: string | null;
  /**
   * Vendor string as the silicon reports it: `"GenuineIntel"`,
   * `"AuthenticAMD"`, or `null` where the platform does not report one — Apple
   * silicon and arm64 Windows commonly do not.
   */
  readonly vendor: string | null;
  /** Instruction set architecture: `"x86_64"`, `"aarch64"`. */
  readonly architecture: string;
  /** Physical cores, or `null` where the platform will not say — which is not 1. */
  readonly physicalCores: number | null;
  /** Logical processors, which is `cores.length`. */
  readonly logicalCores: number;
  /** Usage across the whole package since the previous call, 0–100. */
  readonly usage: number;
  /** Per-core detail, in the order the OS enumerates them. */
  readonly cores: readonly CpuCore[];
}

/** Memory and swap. Every field is bytes; none of the names implies a unit. */
export interface Memory {
  /** Physical memory installed. */
  readonly total: number;
  /**
   * What a new allocation could get. Not `total - used`: it counts reclaimable
   * cache the kernel would evict on demand.
   */
  readonly available: number;
  /** Physical memory in use. */
  readonly used: number;
  /** Swap configured. */
  readonly swapTotal: number;
  /** Swap in use. */
  readonly swapUsed: number;
}

/** A mounted filesystem — what a user means by "a drive". */
export interface Volume {
  /** The device or volume label the OS reports. */
  readonly name: string;
  /** Where it is mounted: `/`, `/home`, `C:\`. */
  readonly mountPoint: string;
  /** Filesystem driver: `"ext4"`, `"apfs"`, `"NTFS"`. */
  readonly fileSystem: string;
  /** What the medium is, where the platform classifies it. */
  readonly kind: "ssd" | "hdd" | "unknown";
  /**
   * Capacity in bytes. Zero for the pseudo-filesystems a running desktop
   * mounts — an AppImage, a snap loopback — which is how to tell them from a
   * real volume.
   */
  readonly total: number;
  /** Free bytes a caller could write. */
  readonly available: number;
  /** Whether the medium can be ejected. */
  readonly removable: boolean;
  /** Whether the mount refuses writes. */
  readonly readOnly: boolean;
}

/** The operating system, and this boot of it. */
export interface Host {
  /** OS name: `"Ubuntu"`, `"Windows"`, `"Darwin"`. */
  readonly name: string | null;
  /** The long form where one exists: `"Ubuntu 24.04.1 LTS"`. */
  readonly longName: string | null;
  /** OS release: `"24.04"`, `"11"`. */
  readonly osVersion: string | null;
  /** Kernel release: `"6.8.0-124-generic"`. */
  readonly kernelVersion: string | null;
  /** `ID` from os-release on Linux; the OS name elsewhere. */
  readonly distributionId: string;
  /** This machine's hostname. */
  readonly hostName: string | null;
  /** Seconds since boot. */
  readonly uptime: number;
  /** Boot time as a Unix timestamp in seconds. */
  readonly bootTime: number;
}

/**
 * `blitsen/os`: what machine this is.
 *
 * None of it has a web spelling. `navigator.hardwareConcurrency` is the closest
 * the platform comes and it answers one deliberately-coarse number; a page
 * cannot ask what the processor is called, how much memory is installed, or
 * what is mounted.
 *
 * Every member is a *reading*, not a constant, and each call samples afresh —
 * so a monitor polls. `cpu().usage` is the share busy since the previous call
 * in particular, which makes the first call the exception: with no earlier call
 * to measure from it reports a baseline against the counters' own origin — on
 * Linux, the average since boot — so a monitor discards it and starts from the
 * second. Every call after the first measures the interval the caller chose.
 *
 * The displays are not here; they are `window.monitors()`.
 */
export interface NativeOs {
  /** Samples the processor. */
  cpu?(): Cpu;
  /** Reads memory and swap. */
  memory?(): Memory;
  /** Lists the mounted volumes. */
  storage?(): Volume[];
  /** Reads the operating system's identity and this boot of it. */
  host?(): Host;
  /** Reads the locale and time zone this session is configured for. */
  locale?(): Locale;
}

/**
 * What the session is localised as, which is what `Intl` defaults to.
 *
 * Both values are what the platform says, and both are what an application
 * hands to a formatter: the tag to `Intl.NumberFormat`, the zone to
 * `Intl.DateTimeFormat`. A machine that states neither reads as `en-US` and
 * `UTC`, which is what the formatters fall back to as well.
 */
export interface Locale {
  /** BCP-47 language tag, such as `en-GB`. */
  language: string;
  /** IANA time zone name, such as `Europe/London`. */
  timeZone: string;
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

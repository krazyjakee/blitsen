/**
 * The `native:` module surfaces, re-exported so an application can name one.
 * The modules themselves are the `blitsen/app` … `blitsen/os` subpaths.
 */
export type {
  ClipboardImage,
  Invocation,
  NativeApp,
  NativeClipboard,
} from "./src/native/native.js";

export interface BlitsenWindowConfig {
  /** Initial presentation. Defaults to `normal`. */
  type?: "normal" | "borderless" | "fullscreen" | "hidden";
  /** Whether the user can resize the window. Defaults to true. */
  resizable?: boolean;
  /** Request a transparent native surface. Support depends on the compositor. */
  transparent?: boolean;
  /** Request that the window stay above normal windows. */
  alwaysOnTop?: boolean;
}

interface BlitsenTrayCommonItem {
  /** Whether the item can be selected. Defaults to true. */
  enabled?: boolean;
}

interface BlitsenTrayActionPresentation extends BlitsenTrayCommonItem {
  /** Native keyboard accelerator, such as `CmdOrCtrl+Shift+KeyO`. */
  accelerator?: string;
  /** PNG displayed beside the item where native menus support icons. */
  icon?: string;
}

export interface BlitsenTrayBuiltinItem extends BlitsenTrayActionPresentation {
  type?: "action";
  action: "show" | "hide" | "quit";
  /** Show, Hide and Quit have matching defaults. */
  label?: string;
  id?: never;
}

export interface BlitsenTrayEventItem extends BlitsenTrayActionPresentation {
  type?: "action";
  id: string;
  label: string;
  action?: never;
}

export type BlitsenTraySeparatorItem =
  | { type: "separator" }
  /** Legacy separator spelling retained for package configuration compatibility. */
  | { action: "separator"; label?: string; enabled?: boolean; type?: never };

export interface BlitsenTrayCheckboxItem extends BlitsenTrayCommonItem {
  type: "checkbox";
  id: string;
  label: string;
  checked?: boolean;
  accelerator?: string;
}

export interface BlitsenTrayRadioItem extends BlitsenTrayCommonItem {
  type: "radio";
  id: string;
  label: string;
  group: string;
  checked?: boolean;
  accelerator?: string;
}

export interface BlitsenTraySubmenuItem extends BlitsenTrayCommonItem {
  type: "submenu";
  label: string;
  icon?: string;
  menu: BlitsenTrayMenuItem[];
}

export type BlitsenTrayMenuItem =
  | BlitsenTrayBuiltinItem
  | BlitsenTrayEventItem
  | BlitsenTraySeparatorItem
  | BlitsenTrayCheckboxItem
  | BlitsenTrayRadioItem
  | BlitsenTraySubmenuItem;

export interface BlitsenTrayConfig {
  /** PNG tray icon, relative to this `package.json`. */
  icon: string;
  tooltip?: string;
  /** Show and focus the window when the tray icon is activated. Defaults to true. */
  openOnClick?: boolean;
  /** Hide the window instead of exiting when its close control is used. */
  closeToTray?: boolean;
  /** Ordered built-in/custom actions, checkable items, separators, and submenus. */
  contextMenu?: BlitsenTrayMenuItem[];
}

export interface BlitsenMenuRoleItem {
  type: "role";
  role: import("./src/native/native.js").MenuRole;
}

export interface BlitsenMenuSeparatorItem {
  type: "separator";
}

export interface BlitsenMenuActionItem {
  type?: "action";
  id: string;
  label: string;
  enabled?: boolean;
  accelerator?: string;
}

export interface BlitsenMenuCheckboxItem {
  type: "checkbox";
  id: string;
  label: string;
  enabled?: boolean;
  checked?: boolean;
  accelerator?: string;
}

export interface BlitsenMenuRadioItem {
  type: "radio";
  id: string;
  label: string;
  group: string;
  enabled?: boolean;
  checked?: boolean;
  accelerator?: string;
}

export interface BlitsenMenuSubmenuItem {
  type: "submenu";
  label: string;
  /** Platform role, valid only on a top-level submenu. */
  role?: import("./src/native/native.js").MenuSubmenuRole;
  enabled?: boolean;
  menu: BlitsenMenuItem[];
}

export type BlitsenMenuItem =
  | BlitsenMenuRoleItem
  | BlitsenMenuSeparatorItem
  | BlitsenMenuActionItem
  | BlitsenMenuCheckboxItem
  | BlitsenMenuRadioItem
  | BlitsenMenuSubmenuItem;

export interface BlitsenMenuConfig {
  /** Top-level submenus, in the order they appear in the bar. */
  menu: BlitsenMenuSubmenuItem[];
}

/** The `blitsen` key of `package.json`, the one place Blitsen reads configuration from. */
export interface BlitsenConfig {
  /**
   * Command run before ingest, from the directory holding this `package.json`.
   * Blitsen runs it and consumes `output`; it never inspects the build tool.
   */
  build?: string;
  /** Directory of static web output, relative to this `package.json`. Must contain `index.html`. */
  output: string;
  /** Application name: the native window title and the default output file name. */
  name?: string;
  /** Native `.node` addons carried into the export, relative to this `package.json`. */
  addons?: string[];
  /** Native window creation options. */
  window?: BlitsenWindowConfig;
  /** System tray icon and context menu. */
  tray?: BlitsenTrayConfig;
  /** Application menu installed at startup; needs no tray icon. */
  menu?: BlitsenMenuConfig;
}

/** The discovered configuration, or `config: null` when no `blitsen` key exists. */
export interface LoadedConfig {
  /** Path of the `package.json` the config came from, or the nearest one found. */
  path: string | null;
  /** Directory holding that `package.json`; `build` runs there and `output` resolves against it. */
  root: string | null;
  config: BlitsenConfig | null;
}

/** JSON Schema for `BlitsenConfig`, published as `blitsen/config.schema.json`. */
export declare const CONFIG_SCHEMA: Record<string, unknown>;

/** Validates a config and returns it, throwing an error naming the offending key. */
export declare function defineConfig(config: BlitsenConfig): BlitsenConfig;

/** Validates a config read from `source`, which is named in any error. */
export declare function validateConfig(config: unknown, source: string): BlitsenConfig;

/** Finds the nearest `package.json` declaring a `blitsen` key, walking up from `from`. */
export declare function loadConfig(from?: string): Promise<LoadedConfig>;

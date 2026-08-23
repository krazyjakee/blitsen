// `blitsen/tray`: the session's system tray icon and menu.
import type { NativeNamespace, NativeTray } from "./native.js";

export type {
  NativeTray,
  RuntimeTrayOptions,
  TrayActionEvent,
  TrayBuiltinAction,
  TrayBuiltinMenuItem,
  TrayCheckboxMenuItem,
  TrayClickEvent,
  TrayEventMenuItem,
  TrayMenuItem,
  TrayRadioMenuItem,
  TraySeparatorMenuItem,
  TraySubmenuItem,
} from "./native.js";

declare const tray: NativeNamespace<NativeTray>;
export default tray;

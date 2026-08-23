// `blitsen/menu`: the application menu, which needs no tray icon.
import type { NativeMenu, NativeNamespace } from "./native.js";

export type {
  ApplicationMenuOptions,
  MenuActionEvent,
  MenuActionItem,
  MenuCheckboxItem,
  MenuItem,
  MenuRadioItem,
  MenuRole,
  MenuRoleItem,
  MenuSeparatorItem,
  MenuSubmenuItem,
  MenuSubmenuRole,
  NativeMenu,
} from "./native.js";

declare const menu: NativeNamespace<NativeMenu>;
export default menu;

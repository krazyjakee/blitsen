// `blitsen/dialog`: the platform file and message dialogs.
import type { NativeDialog, NativeNamespace } from "./native.js";

export type { DialogFilter, FileDialogOptions, MessageDialogOptions, NativeDialog } from "./native.js";

declare const dialog: NativeNamespace<NativeDialog>;
export default dialog;

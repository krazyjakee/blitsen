// `blitsen/shell`: handing a URL or a path to the desktop.
//
// Three operations with three different consequences. `openExternal` sends the
// person to their browser or mail client; `openPath` runs whatever the desktop
// associates with a file, which for a script is the script itself; and
// `showItemInFolder` only shows a file in the file manager, which is the one to
// use for a path the application did not choose. None of them navigates the
// document, and none of them is a shell: the target crosses as one argument.
import type { NativeNamespace, NativeShell } from "./native.js";

export type { NativeShell } from "./native.js";

declare const shell: NativeNamespace<NativeShell>;
export default shell;

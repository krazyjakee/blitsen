// `blitsen/clipboard`: the system clipboard, in the flavours it carries.
import type { NativeClipboard, NativeNamespace } from "./native.js";

export type { ClipboardImage, NativeClipboard } from "./native.js";

declare const clipboard: NativeNamespace<NativeClipboard>;
export default clipboard;

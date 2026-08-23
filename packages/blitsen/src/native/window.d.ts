// `blitsen/window`: the native window belonging to the calling document.
//
// Size, position and scale factor are deliberately not here. Those are
// `innerWidth`, `innerHeight` and `devicePixelRatio`, and `resize` says when
// they changed; a second answer that could disagree with them would be worse
// than no answer. Per-monitor DPI is a different fact, and is in `monitors`.
// `create` is deliberately absent; the isolated multi-window contract is a
// requirement on a future release, not a type promise in this one.
import type { NativeNamespace, NativeWindow } from "./native.js";

export type { CursorGrab, Monitor, NativeWindow } from "./native.js";

declare const window: NativeNamespace<NativeWindow>;
export default window;

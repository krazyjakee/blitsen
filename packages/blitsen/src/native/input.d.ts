// `blitsen/input`: focus-scoped polling state complementary to DOM events.
import type { NativeInput, NativeNamespace } from "./native.js";

export type { NativeInput, NativeInputSnapshot, NativePointerState, PressedKey } from "./native.js";

declare const input: NativeNamespace<NativeInput>;
export default input;

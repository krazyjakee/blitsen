// `blitsen/os`: declared, and empty in this version.
//
// The subpath resolves and every member reads `undefined` inside the runtime,
// so `if (os.something)` is the same feature detection it is on a module with
// members. Naming methods here before the runtime installs them is exactly the
// drift these definitions are checked against, so it names none.
import type { NativeNamespace, NativeUnimplemented } from "./native.js";

declare const os: NativeNamespace<NativeUnimplemented>;
export default os;

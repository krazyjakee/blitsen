// `blitsen/app`: what the application is, rather than what it is showing.
//
// One declaration per subpath, so importing this module offers this module's
// members and no others. The members are checked against the generated API
// manifest, which reads them out of the runtime — see `api-manifest.mjs`.
import type { NativeApp, NativeNamespace } from "./native.js";

export type { Invocation, NativeApp } from "./native.js";

declare const app: NativeNamespace<NativeApp>;
export default app;

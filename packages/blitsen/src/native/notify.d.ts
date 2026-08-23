// `blitsen/notify`: desktop notification submission.
import type { NativeNamespace, NativeNotify } from "./native.js";

export type {
  NativeNotificationAction,
  NativeNotificationEvent,
  NativeNotificationOptions,
  NativeNotificationPermission,
  NativeNotificationUpdate,
  NativeNotify,
} from "./native.js";

declare const notify: NativeNamespace<NativeNotify>;
export default notify;

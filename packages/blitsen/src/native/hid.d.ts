// `blitsen/hid`: raw HID reports for devices that are not ordinary input.
import type { NativeHid, NativeNamespace } from "./native.js";

export type {
  NativeHid,
  NativeHidDevice,
  NativeHidDeviceChange,
  NativeHidDeviceInfo,
  NativeHidInputReport,
  NativeHidUsage,
} from "./native.js";

declare const hid: NativeNamespace<NativeHid>;
export default hid;

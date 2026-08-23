# S10 — raw HID product and backend decision

Status: **go for a narrow desktop module; separate Android implementation; do
not fold HID into `blitsen/input`.**

## Decision

Raw HID is a real native capability with no Node or Web spelling in Blitsen,
but it is not ordinary input. It gets a narrowly named `blitsen/hid` module.
Keyboard, pointer, and controller state remain in DOM events, the standard
Gamepad API, and `blitsen/input`.

The desktop backend should be `hidapi` 2.6.6 or newer:

- Windows uses the native HID class API.
- macOS uses IOHIDManager, with shared access enabled so Blitsen does not claim
  a device exclusively by default.
- Linux uses the Rust-native hidraw backend with `basic-udev`, avoiding a
  runtime dependency on libusb and a build dependency on system `libudev`.
- Android does not share desktop discovery. It uses `UsbManager`, its explicit
  per-device permission result, and file-descriptor wrapping only after the
  Java/Kotlin host has granted access.

The module does not parse report descriptors into controls. Applications ask
for devices by vendor ID, product ID, usage page, and usage, then exchange raw
input, output, and feature reports. Gamepads must use #94 instead.

## Security boundary

Enumeration is not permission to open every collection. The Linux probe
immediately found system keyboard and mouse collections alongside vendor-defined
ones, so an unfiltered `open(path)` would create a keystroke-capture API by
accident.

The public API therefore uses opaque, process-local device IDs and refuses the
Generic Desktop keyboard, keypad, mouse, and pointer usages. It never exposes a
platform device path. Applications cannot opt out of that refusal; a future
specialised accessibility/input product would require a separate decision.

Opening is explicit and asynchronous. Reads happen on a native worker and enter
JavaScript only through the existing FIFO frame-turn queue. Disconnect closes
the handle and produces one terminal event. Output and feature reports have
bounded lengths derived from device capabilities, with a conservative hard
ceiling as a second check.

## Packaging and permissions

| Platform | Requirement | Product behavior |
| --- | --- | --- |
| Linux | A distribution/application udev rule must grant the packaged app access to the intended VID/PID. hidraw nodes and names are udev-controlled. | Return a permission-specific error with the VID/PID and documentation link; never suggest running the app as root and never install a rule at runtime. |
| Windows | User-mode applications open HID top-level collections through the HID class driver. Some system collections are protected. | Filter protected input usages before open and report access-denied separately from disconnect. No driver replacement. |
| macOS | Sandboxed applications need `com.apple.security.device.usb`; shared IOHID access is opt-in in the backend. | The package capability is explicit configuration and is reflected in signing output. Use shared access by default. |
| Android | `UsbManager.requestPermission` may display system UI; a grant lasts until disconnect. Device discovery and opens use the Android USB host API. | `requestDevice()` is lifecycle-aware, survives activity recreation, and returns denial rather than an empty device. No HID module until this host path exists. |

## Proposed API boundary

```ts
interface NativeHid {
  devices(): Promise<readonly HidDeviceInfo[]>;
  open(deviceId: string): Promise<HidDevice>;
  onDeviceChange(listener: (event: HidDeviceChangeEvent) => void): () => void;
}

interface HidDevice {
  readonly id: string;
  readonly opened: boolean;
  receiveFeatureReport(reportId: number): Promise<Uint8Array>;
  sendFeatureReport(data: Uint8Array): Promise<void>;
  write(data: Uint8Array): Promise<void>;
  onInputReport(listener: (event: HidInputReportEvent) => void): () => void;
  close(): Promise<void>;
}
```

`devices()` is a snapshot. `onDeviceChange()` is the hot-plug edge. An input
report includes the report ID separately and data without that leading byte, so
callers do not have to guess whether a platform backend retained it. Public
device IDs are stable only for the process lifetime; serial numbers remain
optional metadata and are never the identity key.

## Linux x64 measurement

The committed probe initialises the selected backend and enumerates usage-page
metadata. On 2026-08-22 it enumerated the attached collections successfully.
The presence of keyboard and mouse usages is the evidence for the mandatory
filter above; the repository does not record device serial numbers or paths.

Rust 1.98.0, release profile with symbols stripped:

| Binary | Installed | gzip -9 |
| --- | ---: | ---: |
| Empty Rust floor | 349,960 B | 170,324 B |
| HID enumeration | 422,704 B | 201,962 B |
| Increment | **72,744 B** | **31,638 B** |

This is a minimal-binary marginal measurement, not the final host delta. It is
small enough that size does not block the desktop module. The selected Linux
dependency tree is `hidapi`, `basic-udev`, `nix`, `libc`, `bitflags`, and
`cfg-if`; the product build still needs its normal cross-target size gate.

## Evidence

- [HIDAPI supports stable Windows, Linux, and macOS backends](https://github.com/libusb/hidapi/wiki),
  and its Linux documentation requires an application udev rule for unprivileged access.
- [The Rust wrapper exposes native Linux and Windows backends and macOS shared access](https://github.com/ruabmbua/hidapi-rs).
- [Linux hidraw is transport-independent and located through udev](https://www.kernel.org/doc/html/latest/hid/hidraw.html).
- [Windows exposes HID top-level collections to user-mode applications](https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/top-level-collections).
- [A sandboxed macOS application needs the USB device entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.device.usb).
- [Android grants USB-device access through `UsbManager.requestPermission`](https://developer.android.com/reference/android/hardware/usb/UsbManager).

## Reproduce

From the repository root on Linux x64:

```sh
cargo build --release --manifest-path spikes/s10/Cargo.toml
target_dir=$(cargo metadata --no-deps --format-version 1 \
  --manifest-path spikes/s10/Cargo.toml \
  | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
"$target_dir/release/s10-hid"
stat -c '%n %s' "$target_dir/release/floor" "$target_dir/release/s10-hid"
gzip -9 -c "$target_dir/release/floor" | wc -c
gzip -9 -c "$target_dir/release/s10-hid" | wc -c
```

The command enumerates metadata only. It does not open a device or send a
report.

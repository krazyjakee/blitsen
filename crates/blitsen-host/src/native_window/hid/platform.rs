//! The `hidapi` backend, and the packaging diagnostics its refusals need.
//!
//! One crate with three native backends, selected in `Cargo.toml` rather than
//! here: the Rust hidraw/`basic-udev` path on Linux, the HID class API through
//! `windows-sys` on Windows, and shared IOHID on macOS. Nothing in this file is
//! conditional on which one is underneath except the sentence a failure adds,
//! because what an application can *do* about a refusal is the one thing that
//! genuinely differs between the three.

use std::ffi::CString;
use std::io::ErrorKind;

use hidapi::{HidApi, HidDevice, HidError};

use super::{BackendDevice, HidBackend, HidHandle};
use crate::dom_bridge::hid::Failure;

/// Where the packaging requirement each platform imposes is written down.
const DOCUMENTATION: &str = "https://blitsen.dev/docs/native-apis/#raw-hid-devices";

/// `hidapi`, created on the first call rather than at startup.
///
/// Constructing it walks the device tree, and an application that never imports
/// `blitsen/hid` must not pay for that — nor touch udev, nor open a IOHIDManager
/// — merely because the runtime was linked with HID support.
#[derive(Default)]
pub(super) struct HidApiBackend {
    api: Option<HidApi>,
    /// Whether the context's device list is newer than the last `enumerate`.
    ///
    /// `HidApi::new` indexes the devices as it initialises, so the first
    /// enumeration after construction already has an answer and refreshing
    /// would walk the tree a second time for nothing.
    indexed: bool,
}

impl HidApiBackend {
    fn api(&mut self) -> Result<&mut HidApi, String> {
        if self.api.is_none() {
            self.api = Some(
                HidApi::new()
                    .map_err(|error| format!("could not initialise HID support: {error}"))?,
            );
            self.indexed = true;
        }
        Ok(self.api.as_mut().expect("the backend was just created"))
    }
}

/// The advice that turns a refusal into something a developer can act on.
fn packaging_advice(vendor_id: u16, product_id: u16) -> String {
    #[cfg(target_os = "linux")]
    {
        format!(
            "hidraw nodes belong to udev, so access is granted by an installed rule rather than \
             by the application: ship one containing \
             `SUBSYSTEM==\"hidraw\", ATTRS{{idVendor}}==\"{vendor_id:04x}\", \
             ATTRS{{idProduct}}==\"{product_id:04x}\", TAG+=\"uaccess\"` and reload udev. \
             Blitsen will not write that rule at run time and must not be run as root. \
             See {DOCUMENTATION}."
        )
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (vendor_id, product_id);
        format!(
            "A sandboxed macOS application must declare `com.apple.security.device.usb` in its \
             entitlements and be signed with them; `blitsen build` writes that entitlements file \
             when the application configures `hid`. See {DOCUMENTATION}."
        )
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (vendor_id, product_id);
        format!(
            "Windows reserves some HID top-level collections for the system and refuses a \
             user-mode open of them; no driver replacement will change that. See {DOCUMENTATION}."
        )
    }
}

/// Classifies an open failure into the four outcomes an application must tell apart.
fn open_failure(device: &BackendDevice, error: &HidError) -> Failure {
    let ids = format!("{:04x}:{:04x}", device.vendor_id, device.product_id);
    let advice = packaging_advice(device.vendor_id, device.product_id);
    match error {
        HidError::IoError { error } => match error.kind() {
            ErrorKind::PermissionDenied => {
                Failure::not_allowed(format!("access to HID device {ids} was denied. {advice}"))
            }
            ErrorKind::NotFound => Failure::not_found(format!(
                "HID device {ids} disappeared before it could be opened"
            )),
            _ => Failure::operation(format!("could not open HID device {ids}: {error}")),
        },
        // Only the Linux and Windows backends report an OS error kind. The
        // macOS one answers a message, so the entitlement — much the most
        // likely cause of a refusal there — is named as guidance rather than
        // asserted as the diagnosis, and the class stays an honest "the
        // backend failed" instead of a permission claim nothing verified.
        other if cfg!(target_os = "macos") => Failure::operation(format!(
            "could not open HID device {ids}: {other}. {advice}"
        )),
        other => Failure::operation(format!("could not open HID device {ids}: {other}")),
    }
}

struct Handle(HidDevice);

impl HidHandle for Handle {
    fn report_descriptor(&self) -> Result<Vec<u8>, String> {
        // The HID specification caps a report descriptor at 4096 bytes, which
        // is what `hidapi` itself allocates for one.
        let mut buffer = vec![0u8; 4096];
        let read = self
            .0
            .get_report_descriptor(&mut buffer)
            .map_err(|error| error.to_string())?;
        buffer.truncate(read);
        Ok(buffer)
    }

    fn write(&self, data: &[u8]) -> Result<(), String> {
        self.0.write(data).map(|_| ()).map_err(|error| error.to_string())
    }

    fn send_feature_report(&self, data: &[u8]) -> Result<(), String> {
        self.0
            .send_feature_report(data)
            .map_err(|error| error.to_string())
    }

    fn get_feature_report(&self, buffer: &mut [u8]) -> Result<usize, String> {
        self.0
            .get_feature_report(buffer)
            .map_err(|error| error.to_string())
    }

    fn read_timeout(&self, buffer: &mut [u8], timeout: i32) -> Result<usize, String> {
        self.0
            .read_timeout(buffer, timeout)
            .map_err(|error| error.to_string())
    }
}

impl HidBackend for HidApiBackend {
    fn enumerate(&mut self) -> Result<Vec<BackendDevice>, String> {
        self.api()?;
        let indexed = std::mem::take(&mut self.indexed);
        let api = self.api()?;
        if !indexed {
            api.refresh_devices()
                .map_err(|error| format!("could not enumerate HID devices: {error}"))?;
        }
        Ok(api
            .device_list()
            .map(|info| BackendDevice {
                path: info.path().to_string_lossy().into_owned(),
                vendor_id: info.vendor_id(),
                product_id: info.product_id(),
                release_number: info.release_number(),
                usage_page: info.usage_page(),
                usage: info.usage(),
                product_name: info.product_string().map(str::to_owned),
                manufacturer_name: info.manufacturer_string().map(str::to_owned),
                serial_number: info.serial_number().map(str::to_owned),
            })
            .collect())
    }

    fn open(&mut self, device: &BackendDevice) -> Result<Option<Box<dyn HidHandle>>, Failure> {
        let path = CString::new(device.path.as_bytes()).map_err(|_| {
            Failure::not_found("that HID device's platform path is no longer valid".into())
        })?;
        let api = self.api().map_err(Failure::operation)?;
        match api.open_path(&path) {
            // Never `None`: this open has already happened by the time it
            // answers. Only Android's has a person in the middle of it.
            Ok(handle) => Ok(Some(Box::new(Handle(handle)))),
            Err(error) => Err(open_failure(device, &error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::native_window::hid::HidController;

    /// The one test that touches the machine's own devices.
    ///
    /// Everything else about this module is driven through an injected backend,
    /// which proves the logic and nothing about the platform. This runs the
    /// real backend against the real device tree: on a developer's Linux box
    /// that tree contains a keyboard and a mouse, which is exactly the case the
    /// filter exists for, and the assertion is that neither survived it and
    /// that nothing identifying the machine reached the snapshot.
    ///
    /// A host with no HID devices at all — a container, a CI runner without
    /// `/sys` — enumerates nothing, and an enumeration that answers nothing is
    /// still the answer this asserts about. A backend that cannot initialise
    /// there is reported rather than failed, because "this machine has no HID
    /// support" is a fact about the machine and not a regression in the code.
    #[test]
    fn the_real_backend_enumerates_this_machine_without_leaking_it() {
        let mut backend = HidApiBackend::default();
        let enumerated = match backend.enumerate() {
            Ok(devices) => devices,
            Err(error) => {
                println!("HID smoke skipped: {error}");
                return;
            }
        };
        let mut controller =
            HidController::with_backend(Box::new(HidApiBackend::default()), Arc::new(|| {}));
        let snapshot = controller
            .devices()
            .expect("the real backend enumerates without failing");
        let devices = snapshot.as_array().expect("devices answers an array");
        println!(
            "HID smoke: {} top-level collections, {} openable devices",
            enumerated.len(),
            devices.len()
        );
        for device in devices {
            for usage in device["usages"].as_array().expect("usages is an array") {
                let usage_page = usage["usagePage"].as_u64().expect("a usage page");
                let usage_id = usage["usage"].as_u64().expect("a usage");
                assert!(
                    !super::super::protected(usage_page as u16, usage_id as u16),
                    "a protected collection reached the public snapshot: {device}"
                );
            }
            assert!(
                device["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with('d') && id[1..].parse::<u64>().is_ok()),
                "device ids are opaque counters: {device}"
            );
        }
        let rendered = serde_json::to_string(&snapshot).expect("the snapshot serializes");
        for path in enumerated.iter().map(|device| device.path.as_str()) {
            assert!(!rendered.contains(path), "{rendered} carries a platform path");
        }
    }
}

//! `blitsen/hid` over Android's USB host API and its per-device permission (#248).
//!
//! Android does not have hidraw, IOHID or a HID class driver, and it does not
//! have desktop discovery either. It has `UsbManager`: a list of attached USB
//! devices anyone may read, and access to any one of them that only exists once
//! a person has granted it to a system dialog naming that device. So the two
//! things this file does that the desktop backend does not are enumerate HID
//! *interfaces* out of USB descriptors, and turn "the dialog is up" into the
//! answer the shared controller already understands: `Ok(None)`, an open with
//! no answer yet.
//!
//! Everything after the handle exists is deliberately identical to desktop.
//! Report framing is the wire behaviour `hidapi`'s own libusb backend produces,
//! because that is what the desktop backends produce and #247's public contract
//! is written in it: an output report is the interrupt OUT endpoint where the
//! interface has one and a `SET_REPORT` control transfer where it does not, a
//! feature report is `GET_REPORT`/`SET_REPORT` on the control endpoint, and a
//! leading report ID of zero means "this device does not use report IDs" and is
//! stripped from the wire rather than sent.
//!
//! ## Why HID has no `BroadcastReceiver`
//!
//! The documented shape of `UsbManager.requestPermission` is a `PendingIntent`
//! whose broadcast a `BroadcastReceiver` catches. The APK's deliberately small
//! dex contains only notification activation callbacks (#252), not a second
//! permission protocol for HID. Adding one is unnecessary because the answer
//! can be read without owning the broadcast, as described below.
//!
//! What replaces it is the state the broadcast would have reported, read from
//! the source the system keeps anyway: `UsbManager.hasPermission`. That is
//! strictly *more* lifecycle-safe than a receiver, and this is the argument for
//! it rather than an apology. A registered receiver dies with the activity that
//! registered it, so a grant that arrives during a recreation is delivered to
//! nothing; `hasPermission` is the system's own record of the grant, survives
//! recreation because it was never ours to lose, and is re-read on the next
//! frame turn whatever happened to the activity in between. The `PendingIntent`
//! is still constructed because the API requires one, and it is immutable
//! because nothing here reads the extras the system would fill in.
//!
//! A denial has no such record — the system remembers "not granted", which is
//! also what it says before anyone was asked. So a denial is concluded the way
//! [`crate::native_window::notify`] concludes one for `POST_NOTIFICATIONS`: the
//! dialog takes window focus, and focus coming back with the permission still
//! ungranted is the user having dismissed it. Same platform, same limitation,
//! same reading of it, rather than a second invention.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::dom_bridge::hid::Failure;

use super::{BackendDevice, HidBackend, HidHandle};

/// The JNI half: `UsbManager`, `UsbDeviceConnection` and nothing else.
#[cfg(target_os = "android")]
pub(crate) mod usb;

/// The USB interface class that means "human interface device".
const HID_CLASS: i32 = 3;
/// Subclass 1 is the boot interface, the only subclass the specification defines.
const BOOT_SUBCLASS: i32 = 1;
/// Boot protocol 1 is a keyboard and 2 is a mouse.
const BOOT_KEYBOARD: i32 = 1;
const BOOT_MOUSE: i32 = 2;

/// The report types a HID `SET_REPORT`/`GET_REPORT` request addresses.
const REPORT_TYPE_OUTPUT: u8 = 2;
const REPORT_TYPE_FEATURE: u8 = 3;

/// The Generic Desktop usages a boot interface stands for.
///
/// A boot keyboard and a boot mouse are exactly the collections [`super`]
/// refuses, so they are enumerated *as* those usages rather than filtered out
/// here: an id that was refused has to stay distinguishable from one nobody
/// ever issued, and the shared scan is what draws that line.
const KEYBOARD_USAGE: (u16, u16) = (0x01, 0x06);
const MOUSE_USAGE: (u16, u16) = (0x01, 0x02);

/// How long a permission dialog may be up before silence is read as a refusal.
///
/// The fallback for the focus signal, not the primary one. Two seconds is long
/// enough that a dialog which is genuinely being read has already taken focus,
/// and short enough that an application does not wait forever for an answer the
/// platform will never volunteer.
const PROMPT_GRACE: Duration = Duration::from_secs(2);

/// How long an output report may take before the write is reported as failed.
///
/// A HID interrupt endpoint is polled at its declared interval, so a write that
/// has not gone in a quarter of a second is not slow, it is not happening. The
/// worker is blocked for that long at worst, and the frame loop is not.
const WRITE_TIMEOUT: i32 = 250;

/// How often a silent endpoint is checked against the device list.
///
/// `UsbDeviceConnection.bulkTransfer` answers `-1` for a timeout and `-1` for a
/// device that has gone, which is the one thing the desktop backends never make
/// this module guess about. Enumerating to tell them apart is a JNI call into a
/// system service, and the read loop times out 125 times a second, so it is
/// worth doing at human timescale and not at read timescale: a second is far
/// below noticing a cable came out and 125 times cheaper than the alternative.
const LIVENESS_INTERVAL: Duration = Duration::from_secs(1);

/// One USB HID interface, as the Android USB host API describes it.
///
/// `device_name` is `UsbDevice.getDeviceName()` — the `/dev/bus/usb` node — and
/// is the permission key as well as half of the private path. It is a platform
/// path in exactly S10's sense and never reaches an application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsbInterface {
    pub(crate) device_name: String,
    pub(crate) interface_id: i32,
    pub(crate) vendor_id: u16,
    pub(crate) product_id: u16,
    /// `UsbDevice.getVersion()`, which renders `bcdDevice` as `"1.00"`.
    pub(crate) version: Option<String>,
    pub(crate) interface_class: i32,
    pub(crate) interface_subclass: i32,
    pub(crate) interface_protocol: i32,
    pub(crate) product_name: Option<String>,
    pub(crate) manufacturer_name: Option<String>,
    /// Present only once this device's permission has been granted.
    ///
    /// `UsbDevice.getSerialNumber` throws `SecurityException` without the grant
    /// from API 29, so an enumeration of a device nobody has been asked about
    /// cannot read one and does not try. S10 wanted a serial number to be
    /// metadata rather than identity; on Android it is not even available until
    /// after the identity — the opaque public id — has already been issued.
    pub(crate) serial_number: Option<String>,
}

impl UsbInterface {
    /// The private path the shared controller groups and identifies devices by.
    ///
    /// The interface, not the device. A USB interface is what Android claims,
    /// what carries its own endpoints, and what has one report descriptor — so
    /// it is the exact counterpart of the hidraw node or top-level collection
    /// the desktop backends hand over, and the composite rule lands in the same
    /// place: one report descriptor, every top-level collection inside it, all
    /// of them refused together if any one is protected.
    fn path(&self) -> String {
        format!("{}#{}", self.device_name, self.interface_id)
    }

    /// The Generic Desktop usage this interface declares before it is opened.
    ///
    /// Only a boot interface declares one. Everything else answers `(0, 0)`,
    /// because a USB descriptor does not carry usages — they are in the HID
    /// report descriptor, which cannot be read without an open, which cannot
    /// happen without permission. Inventing a usage there would be a claim the
    /// platform never made; the real usages are checked against the descriptor
    /// the moment there is a handle to read one through.
    fn usage(&self) -> (u16, u16) {
        if self.interface_subclass != BOOT_SUBCLASS {
            return (0, 0);
        }
        match self.interface_protocol {
            BOOT_KEYBOARD => KEYBOARD_USAGE,
            BOOT_MOUSE => MOUSE_USAGE,
            _ => (0, 0),
        }
    }
}

/// `bcdDevice` back out of the string Android renders it as.
///
/// `UsbDevice.getVersion()` formats the binary-coded-decimal release as
/// `"%x.%02x"`, so the way back is to read both halves as hexadecimal. The
/// desktop backends report the raw `bcdDevice`, and an application comparing
/// `releaseNumber` across platforms has to be comparing the same number.
fn release_number(version: Option<&str>) -> u16 {
    let Some((major, minor)) = version.and_then(|version| version.split_once('.')) else {
        return 0;
    };
    let major = u16::from_str_radix(major, 16).unwrap_or(0);
    let minor = u16::from_str_radix(minor, 16).unwrap_or(0);
    (major << 8) | (minor & 0xff)
}

/// Raw transfers on one claimed interface. The JNI seam for report exchange.
pub(crate) trait UsbConnection: Send {
    /// The HID report descriptor, through a `GET_DESCRIPTOR` control transfer.
    fn report_descriptor(&self) -> Result<Vec<u8>, String>;
    /// Reads the interrupt IN endpoint, answering `None` when nothing arrived.
    fn interrupt_in(&self, buffer: &mut [u8], timeout: i32) -> Result<Option<usize>, String>;
    /// Writes the interrupt OUT endpoint, answering `false` if there is none.
    fn interrupt_out(&self, data: &[u8], timeout: i32) -> Result<bool, String>;
    /// A `SET_REPORT` control transfer of the given report type and ID.
    fn set_report(&self, report_type: u8, report_id: u8, data: &[u8]) -> Result<(), String>;
    /// A `GET_REPORT` control transfer of the given report type and ID.
    fn get_report(
        &self,
        report_type: u8,
        report_id: u8,
        buffer: &mut [u8],
    ) -> Result<usize, String>;
    /// Whether the device this interface belongs to is still attached.
    fn attached(&self) -> Result<bool, String>;
}

/// Enumeration, permission and opening. The JNI seam for `UsbManager` itself.
pub(crate) trait UsbApi: Send {
    /// The claimed interface this API hands back, named rather than boxed so
    /// the handle below stays one concrete type per platform.
    type Connection: UsbConnection + 'static;

    /// Every interface of every attached USB device, HID or not.
    fn interfaces(&mut self) -> Result<Vec<UsbInterface>, String>;
    /// `UsbManager.hasPermission` for the device this interface belongs to.
    fn has_permission(&mut self, device_name: &str) -> Result<bool, String>;
    /// Raises the system dialog through `UsbManager.requestPermission`.
    fn request_permission(&mut self, device_name: &str) -> Result<(), String>;
    /// Whether the activity holds window focus, which the dialog takes.
    fn focused(&mut self) -> Result<bool, String>;
    /// Opens and claims the interface, which requires the grant to be held.
    fn open(&mut self, interface: &UsbInterface) -> Result<Self::Connection, String>;
}

/// A permission dialog that has been raised and not yet answered.
struct Prompt {
    started: Instant,
    /// Whether the activity has lost focus since the dialog was raised.
    saw_focus_loss: bool,
}

/// The `blitsen/hid` backend for Android.
pub(crate) struct UsbHidBackend<A: UsbApi> {
    api: A,
    /// The last enumeration, by private path, so an open knows what to claim.
    seen: HashMap<String, UsbInterface>,
    /// Dialogs in flight, by device rather than by interface: the grant Android
    /// records is the device's, so two interfaces of one composite device are
    /// one question to the person answering it.
    prompts: HashMap<String, Prompt>,
}

impl<A: UsbApi> UsbHidBackend<A> {
    pub(crate) fn new(api: A) -> Self {
        Self {
            api,
            seen: HashMap::new(),
            prompts: HashMap::new(),
        }
    }

    /// Decides what a device without a grant means for the open asking for one.
    ///
    /// The first call raises the dialog; later calls read the focus signal that
    /// says the dialog has been up and is gone. A concluded denial forgets the
    /// prompt, which is what makes "denied requests can be made again" true —
    /// the next `open()` asks the platform again rather than replaying a stored
    /// no that the user may since have changed their mind about.
    fn awaiting(
        &mut self,
        device_name: &str,
        ids: &str,
    ) -> Result<Option<Box<dyn HidHandle>>, Failure> {
        if !self.prompts.contains_key(device_name) {
            self.api
                .request_permission(device_name)
                .map_err(Failure::operation)?;
            self.prompts.insert(
                device_name.to_owned(),
                Prompt {
                    started: Instant::now(),
                    saw_focus_loss: false,
                },
            );
            return Ok(None);
        }
        let focused = self.api.focused().map_err(Failure::operation)?;
        let prompt = self
            .prompts
            .get_mut(device_name)
            .expect("the prompt was just found");
        if !focused {
            prompt.saw_focus_loss = true;
            return Ok(None);
        }
        if !prompt.saw_focus_loss && prompt.started.elapsed() < PROMPT_GRACE {
            return Ok(None);
        }
        self.prompts.remove(device_name);
        Err(Failure::not_allowed(format!(
            "permission to use USB device {ids} was not granted. Android grants USB access one \
             device at a time and only while it is attached; opening it again raises the system \
             dialog again."
        )))
    }
}

impl<A: UsbApi> HidBackend for UsbHidBackend<A> {
    fn enumerate(&mut self) -> Result<Vec<BackendDevice>, String> {
        let interfaces = self.api.interfaces()?;
        self.seen.clear();
        let mut devices = Vec::new();
        for interface in interfaces {
            if interface.interface_class != HID_CLASS {
                continue;
            }
            let (usage_page, usage) = interface.usage();
            devices.push(BackendDevice {
                path: interface.path(),
                vendor_id: interface.vendor_id,
                product_id: interface.product_id,
                release_number: release_number(interface.version.as_deref()),
                usage_page,
                usage,
                product_name: interface.product_name.clone(),
                manufacturer_name: interface.manufacturer_name.clone(),
                serial_number: interface.serial_number.clone(),
            });
            self.seen.insert(interface.path(), interface);
        }
        // A device that is gone cannot be waiting for an answer, and its grant
        // died with it: Android revokes a USB permission when the device is
        // detached. Dropping the prompt here is what makes a reattached device
        // ask again rather than wait on a dialog that is no longer up.
        let seen = &self.seen;
        self.prompts.retain(|device_name, _| {
            seen.values()
                .any(|interface| &interface.device_name == device_name)
        });
        Ok(devices)
    }

    fn open(&mut self, device: &BackendDevice) -> Result<Option<Box<dyn HidHandle>>, Failure> {
        let Some(interface) = self.seen.get(&device.path).cloned() else {
            return Err(Failure::not_found(format!(
                "USB device {:04x}:{:04x} is no longer attached",
                device.vendor_id, device.product_id
            )));
        };
        let ids = format!("{:04x}:{:04x}", device.vendor_id, device.product_id);
        // Asked afresh rather than taken from the enumeration: this is the call
        // that observes a grant made since, including one made while the
        // activity was being recreated, and it is the only thing that decides
        // whether a handle may exist.
        if !self
            .api
            .has_permission(&interface.device_name)
            .map_err(Failure::operation)?
        {
            return self.awaiting(&interface.device_name, &ids);
        }
        self.prompts.remove(&interface.device_name);
        let connection = self.api.open(&interface).map_err(|error| {
            // The grant is held, so a refusal here is the device going away
            // between the two calls or the service failing — never permission,
            // which the check above already settled.
            Failure::operation(format!("could not open USB device {ids}: {error}"))
        })?;
        Ok(Some(Box::new(UsbHidHandle::new(connection))))
    }
}

/// One claimed HID interface, framed the way the desktop backends frame reports.
pub(crate) struct UsbHidHandle<C: UsbConnection> {
    connection: C,
    /// When the device list was last consulted about a silent endpoint.
    ///
    /// `Mutex` because [`HidHandle`] takes `&self` — the handle is owned by one
    /// worker thread and never shared, so this is uncontended and exists only
    /// to keep the trait's shape, which is `hidapi`'s.
    checked: parking_lot::Mutex<Option<Instant>>,
}

impl<C: UsbConnection> UsbHidHandle<C> {
    fn new(connection: C) -> Self {
        Self {
            connection,
            checked: parking_lot::Mutex::new(None),
        }
    }
}

/// Splits a report into the ID a control transfer addresses and its wire bytes.
///
/// `hidapi` takes a report whose first byte is the report ID and, when that
/// byte is zero, sends the rest without it — zero is not a report ID, it is
/// this device saying it has none. Both desktop backends do that, the public
/// contract is written in it, and a device that got its leading zero on the
/// wire would see a report one byte longer than it declared.
fn framed(data: &[u8]) -> (u8, &[u8]) {
    match data.split_first() {
        Some((&0, rest)) => (0, rest),
        Some((&report_id, _)) => (report_id, data),
        None => (0, data),
    }
}

impl<C: UsbConnection> HidHandle for UsbHidHandle<C> {
    fn report_descriptor(&self) -> Result<Vec<u8>, String> {
        self.connection.report_descriptor()
    }

    fn write(&self, data: &[u8]) -> Result<(), String> {
        let (report_id, wire) = framed(data);
        // The interrupt endpoint first, exactly as `hidapi` does: it is the
        // path a device's own firmware expects an output report on, and the
        // control transfer is the fallback for an interface that has no OUT
        // endpoint at all rather than an equivalent way of doing it.
        if self.connection.interrupt_out(wire, WRITE_TIMEOUT)? {
            return Ok(());
        }
        self.connection
            .set_report(REPORT_TYPE_OUTPUT, report_id, wire)
    }

    fn send_feature_report(&self, data: &[u8]) -> Result<(), String> {
        let (report_id, wire) = framed(data);
        self.connection
            .set_report(REPORT_TYPE_FEATURE, report_id, wire)
    }

    fn get_feature_report(&self, buffer: &mut [u8]) -> Result<usize, String> {
        // The caller's buffer starts with the report ID it is asking for, and
        // the answer it wants back has that byte in front of the payload — the
        // shape `hidapi` produces and the shape the controller strips one byte
        // off before it reaches an application.
        let report_id = buffer.first().copied().unwrap_or(0);
        let read = self
            .connection
            .get_report(REPORT_TYPE_FEATURE, report_id, &mut buffer[1..])?;
        Ok(read + 1)
    }

    fn read_timeout(&self, buffer: &mut [u8], timeout: i32) -> Result<usize, String> {
        if let Some(read) = self.connection.interrupt_in(buffer, timeout)? {
            *self.checked.lock() = None;
            return Ok(read);
        }
        // Nothing arrived, which Android reports the same way whether the
        // device is idle or gone. Idle is overwhelmingly the common case, so it
        // is the one that costs nothing; the question is asked at most once a
        // second, and only of a device that has been silent for that long.
        let mut checked = self.checked.lock();
        if checked.is_some_and(|last| last.elapsed() < LIVENESS_INTERVAL) {
            return Ok(0);
        }
        *checked = Some(Instant::now());
        if self.connection.attached()? {
            return Ok(0);
        }
        Err("the USB device was detached".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use parking_lot::Mutex;

    /// A `UsbManager` with no Android under it.
    #[derive(Default)]
    struct FakeUsb {
        interfaces: Arc<Mutex<Vec<UsbInterface>>>,
        granted: Arc<Mutex<Vec<String>>>,
        /// Every `requestPermission`, so a second dialog is a visible failure.
        requested: Arc<Mutex<Vec<String>>>,
        focused: Arc<Mutex<bool>>,
        descriptor: Vec<u8>,
    }

    /// Every control transfer out, as `(report type, report id, payload)`.
    type ControlLog = Arc<Mutex<Vec<(u8, u8, Vec<u8>)>>>;

    #[derive(Default)]
    struct FakeConnection {
        descriptor: Vec<u8>,
        control: ControlLog,
        interrupt: Arc<Mutex<Vec<Vec<u8>>>>,
        has_out_endpoint: bool,
        feature: Arc<Mutex<Vec<u8>>>,
        input: Arc<Mutex<Vec<Vec<u8>>>>,
        attached: Arc<Mutex<bool>>,
    }

    impl UsbConnection for FakeConnection {
        fn report_descriptor(&self) -> Result<Vec<u8>, String> {
            Ok(self.descriptor.clone())
        }

        fn interrupt_in(&self, buffer: &mut [u8], _timeout: i32) -> Result<Option<usize>, String> {
            let mut input = self.input.lock();
            if input.is_empty() {
                return Ok(None);
            }
            let report = input.remove(0);
            let len = report.len().min(buffer.len());
            buffer[..len].copy_from_slice(&report[..len]);
            Ok(Some(len))
        }

        fn interrupt_out(&self, data: &[u8], _timeout: i32) -> Result<bool, String> {
            if !self.has_out_endpoint {
                return Ok(false);
            }
            self.interrupt.lock().push(data.to_vec());
            Ok(true)
        }

        fn set_report(&self, report_type: u8, report_id: u8, data: &[u8]) -> Result<(), String> {
            self.control
                .lock()
                .push((report_type, report_id, data.to_vec()));
            if report_type == REPORT_TYPE_FEATURE {
                *self.feature.lock() = data.to_vec();
            }
            Ok(())
        }

        fn get_report(
            &self,
            _report_type: u8,
            _report_id: u8,
            buffer: &mut [u8],
        ) -> Result<usize, String> {
            let stored = self.feature.lock().clone();
            let len = stored.len().min(buffer.len());
            buffer[..len].copy_from_slice(&stored[..len]);
            Ok(len)
        }

        fn attached(&self) -> Result<bool, String> {
            Ok(*self.attached.lock())
        }
    }

    impl UsbApi for FakeUsb {
        fn interfaces(&mut self) -> Result<Vec<UsbInterface>, String> {
            Ok(self.interfaces.lock().clone())
        }

        fn has_permission(&mut self, device_name: &str) -> Result<bool, String> {
            Ok(self.granted.lock().iter().any(|name| name == device_name))
        }

        fn request_permission(&mut self, device_name: &str) -> Result<(), String> {
            self.requested.lock().push(device_name.to_owned());
            Ok(())
        }

        fn focused(&mut self) -> Result<bool, String> {
            Ok(*self.focused.lock())
        }

        type Connection = FakeConnection;

        fn open(&mut self, _interface: &UsbInterface) -> Result<FakeConnection, String> {
            Ok(FakeConnection {
                descriptor: self.descriptor.clone(),
                attached: Arc::new(Mutex::new(true)),
                ..FakeConnection::default()
            })
        }
    }

    fn interface(device_name: &str, id: i32, subclass: i32, protocol: i32) -> UsbInterface {
        UsbInterface {
            device_name: device_name.into(),
            interface_id: id,
            vendor_id: 0x16c0,
            product_id: 0x27dc,
            version: Some("1.00".into()),
            interface_class: HID_CLASS,
            interface_subclass: subclass,
            interface_protocol: protocol,
            product_name: Some("Widget".into()),
            manufacturer_name: Some("Acme".into()),
            serial_number: None,
        }
    }

    fn backend(interfaces: Vec<UsbInterface>) -> (UsbHidBackend<FakeUsb>, FakeUsb) {
        let api = FakeUsb {
            interfaces: Arc::new(Mutex::new(interfaces)),
            granted: Arc::new(Mutex::new(Vec::new())),
            requested: Arc::new(Mutex::new(Vec::new())),
            focused: Arc::new(Mutex::new(true)),
            descriptor: Vec::new(),
        };
        let handle = FakeUsb {
            interfaces: Arc::clone(&api.interfaces),
            granted: Arc::clone(&api.granted),
            requested: Arc::clone(&api.requested),
            focused: Arc::clone(&api.focused),
            descriptor: Vec::new(),
        };
        (UsbHidBackend::new(api), handle)
    }

    #[test]
    fn usb_descriptors_map_onto_the_shared_public_device_information() {
        let (mut backend, _) = backend(vec![
            interface("/dev/bus/usb/001/002", 0, 0, 0),
            // Not HID, so not this module's business at all.
            UsbInterface {
                interface_class: 8,
                ..interface("/dev/bus/usb/001/003", 0, 0, 0)
            },
        ]);
        let devices = backend.enumerate().expect("enumeration succeeds");
        assert_eq!(
            devices,
            vec![BackendDevice {
                path: "/dev/bus/usb/001/002#0".into(),
                vendor_id: 0x16c0,
                product_id: 0x27dc,
                release_number: 0x0100,
                // Not knowable before the report descriptor is readable, and
                // the platform is not asked to guess.
                usage_page: 0,
                usage: 0,
                product_name: Some("Widget".into()),
                manufacturer_name: Some("Acme".into()),
                serial_number: None,
            }]
        );
        // `getVersion` is `%x.%02x` over `bcdDevice`, so both halves are hex.
        assert_eq!(release_number(Some("2.10")), 0x0210);
        assert_eq!(release_number(Some("1.00")), 0x0100);
        assert_eq!(release_number(None), 0);
    }

    #[test]
    fn a_boot_keyboard_or_mouse_is_enumerated_as_the_usage_that_refuses_it() {
        let (mut backend, _) = backend(vec![
            interface("/dev/bus/usb/001/002", 0, BOOT_SUBCLASS, BOOT_KEYBOARD),
            interface("/dev/bus/usb/001/003", 0, BOOT_SUBCLASS, BOOT_MOUSE),
            interface("/dev/bus/usb/001/004", 1, BOOT_SUBCLASS, 0),
            interface("/dev/bus/usb/001/005", 2, 0, 0),
        ]);
        let usages = backend
            .enumerate()
            .expect("enumeration succeeds")
            .iter()
            .map(|device| (device.usage_page, device.usage))
            .collect::<Vec<_>>();
        assert_eq!(
            usages,
            vec![KEYBOARD_USAGE, MOUSE_USAGE, (0, 0), (0, 0)],
            "a boot interface names the collection the shared filter refuses"
        );
        // And the shared filter is what actually refuses it, without this
        // module repeating the rule: `super::protected` is the one decision.
        assert!(super::super::protected(KEYBOARD_USAGE.0, KEYBOARD_USAGE.1));
        assert!(super::super::protected(MOUSE_USAGE.0, MOUSE_USAGE.1));
        assert!(!super::super::protected(0, 0));
    }

    #[test]
    fn one_dialog_is_raised_and_a_grant_opens_the_device_it_named() {
        let (mut backend, api) = backend(vec![interface("/dev/bus/usb/001/002", 0, 0, 0)]);
        let devices = backend.enumerate().expect("enumeration succeeds");
        let target = &devices[0];

        for _ in 0..5 {
            assert!(
                backend
                    .open(target)
                    .expect("the request is outstanding")
                    .is_none(),
                "an unanswered dialog leaves the open unsettled"
            );
        }
        assert_eq!(
            *api.requested.lock(),
            vec!["/dev/bus/usb/001/002"],
            "five turns of waiting must raise one dialog"
        );

        api.granted.lock().push("/dev/bus/usb/001/002".into());
        assert!(
            backend.open(target).expect("the grant opens it").is_some(),
            "a granted device produces a handle on the turn that sees the grant"
        );
        assert_eq!(
            api.requested.lock().len(),
            1,
            "the grant is observed, not asked for again"
        );
    }

    #[test]
    fn a_dismissed_dialog_is_a_denial_that_can_be_asked_again() {
        let (mut backend, api) = backend(vec![interface("/dev/bus/usb/001/002", 0, 0, 0)]);
        let devices = backend.enumerate().expect("enumeration succeeds");
        let target = &devices[0];

        assert!(matches!(backend.open(target), Ok(None)));
        // The dialog takes focus, and focus coming back with nothing granted is
        // the only signal Android gives that the question was answered "no".
        *api.focused.lock() = false;
        assert!(matches!(backend.open(target), Ok(None)));
        *api.focused.lock() = true;
        let Err(denied) = backend.open(target) else {
            panic!("a dismissed dialog is a denial, not a handle");
        };
        assert_eq!(denied.name, "NotAllowedError");

        // Asking again raises the dialog again rather than replaying the no.
        assert!(matches!(backend.open(target), Ok(None)));
        assert_eq!(api.requested.lock().len(), 2);
    }

    #[test]
    fn a_grant_survives_recreation_and_dies_with_the_device() {
        let interfaces = vec![interface("/dev/bus/usb/001/002", 0, 0, 0)];
        let (mut backend, api) = backend(interfaces.clone());
        let devices = backend.enumerate().expect("enumeration succeeds");
        let target = devices[0].clone();
        assert!(matches!(backend.open(&target), Ok(None)));

        // Activity recreation: everything Blitsen held is rebuilt, and the
        // grant the user made in the meantime is the system's own record. A
        // backend that had kept the answer in a receiver it registered would
        // have lost it here; this one reads it and never asks twice.
        api.granted.lock().push("/dev/bus/usb/001/002".into());
        let mut recreated = UsbHidBackend::new(FakeUsb {
            interfaces: Arc::clone(&api.interfaces),
            granted: Arc::clone(&api.granted),
            requested: Arc::clone(&api.requested),
            focused: Arc::clone(&api.focused),
            descriptor: Vec::new(),
        });
        recreated.enumerate().expect("enumeration succeeds");
        assert!(matches!(recreated.open(&target), Ok(Some(_))));
        assert_eq!(
            api.requested.lock().len(),
            1,
            "recreation must not raise a second dialog for a granted device"
        );

        // Unplugged: Android revokes the grant with the device, and the id that
        // named it is now a device nobody can find rather than one refused.
        api.interfaces.lock().clear();
        api.granted.lock().clear();
        recreated.enumerate().expect("enumeration succeeds");
        let Err(gone) = recreated.open(&target) else {
            panic!("a detached device cannot be opened");
        };
        assert_eq!(gone.name, "NotFoundError");

        // Reattached: the same device is a fresh question, not a stale answer.
        *api.interfaces.lock() = interfaces;
        recreated.enumerate().expect("enumeration succeeds");
        assert!(matches!(recreated.open(&target), Ok(None)));
        assert_eq!(api.requested.lock().len(), 2);
    }

    #[test]
    fn reports_are_framed_the_way_the_desktop_backends_frame_them() {
        let control = Arc::new(Mutex::new(Vec::new()));
        let interrupt = Arc::new(Mutex::new(Vec::new()));
        let feature = Arc::new(Mutex::new(Vec::new()));
        let handle = UsbHidHandle::new(FakeConnection {
            control: Arc::clone(&control),
            interrupt: Arc::clone(&interrupt),
            feature: Arc::clone(&feature),
            has_out_endpoint: true,
            attached: Arc::new(Mutex::new(true)),
            ..FakeConnection::default()
        });
        // A device that uses report IDs keeps the leading byte on the wire.
        handle.write(&[0x03, 0x42]).expect("the report goes out");
        // A device that does not is spelled `0` by the caller, and zero is not
        // a report ID — it is the absence of one, and must not be transmitted.
        handle.write(&[0x00, 0x42]).expect("the report goes out");
        assert_eq!(*interrupt.lock(), vec![vec![0x03, 0x42], vec![0x42]]);
        assert!(
            control.lock().is_empty(),
            "an interface with an OUT endpoint never falls back to the control pipe"
        );

        handle
            .send_feature_report(&[0x05, 0x7f])
            .expect("the feature report goes out");
        assert_eq!(
            *control.lock(),
            vec![(REPORT_TYPE_FEATURE, 0x05, vec![0x05, 0x7f])]
        );
        let mut buffer = vec![0u8; 8];
        buffer[0] = 0x05;
        let read = handle
            .get_feature_report(&mut buffer)
            .expect("the feature report comes back");
        assert_eq!(
            (read, &buffer[..read]),
            (3, &[0x05, 0x05, 0x7f][..]),
            "the answer keeps the report ID in front, as hidapi's does"
        );
    }

    #[test]
    fn an_interface_without_an_out_endpoint_writes_through_the_control_pipe() {
        let control = Arc::new(Mutex::new(Vec::new()));
        let handle = UsbHidHandle::new(FakeConnection {
            control: Arc::clone(&control),
            has_out_endpoint: false,
            attached: Arc::new(Mutex::new(true)),
            ..FakeConnection::default()
        });
        handle.write(&[0x03, 0x42]).expect("the report goes out");
        handle.write(&[0x00, 0x42]).expect("the report goes out");
        assert_eq!(
            *control.lock(),
            vec![
                (REPORT_TYPE_OUTPUT, 0x03, vec![0x03, 0x42]),
                (REPORT_TYPE_OUTPUT, 0x00, vec![0x42]),
            ]
        );
    }

    #[test]
    fn a_silent_endpoint_is_a_timeout_until_the_device_is_gone() {
        let attached = Arc::new(Mutex::new(true));
        let input = Arc::new(Mutex::new(vec![vec![0x03, 0x01]]));
        let handle = UsbHidHandle::new(FakeConnection {
            input: Arc::clone(&input),
            attached: Arc::clone(&attached),
            ..FakeConnection::default()
        });
        let mut buffer = vec![0u8; 8];
        assert_eq!(handle.read_timeout(&mut buffer, 8), Ok(2));
        // Nothing to read is not a disconnect, and the same answer twice does
        // not become one: an idle device reads like this 125 times a second.
        assert_eq!(handle.read_timeout(&mut buffer, 8), Ok(0));
        assert_eq!(handle.read_timeout(&mut buffer, 8), Ok(0));

        *attached.lock() = false;
        // The liveness question was asked on the first silent read, so it is
        // not asked again until the interval is up — the check is deliberately
        // not on the read path's hot loop.
        assert_eq!(handle.read_timeout(&mut buffer, 8), Ok(0));
        *handle.checked.lock() = None;
        assert!(
            handle.read_timeout(&mut buffer, 8).is_err(),
            "a detached device ends the worker, which is the one terminal event"
        );
    }
}

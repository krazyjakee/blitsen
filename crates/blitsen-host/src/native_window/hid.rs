//! Raw HID enumeration, report exchange, and hot-plug for `blitsen/hid` (#247).
//!
//! The security boundary S10 drew is the reason this module exists at all.
//! Enumerating HID on any desktop immediately finds the system keyboard and
//! mouse beside the vendor-defined collections an application actually wants,
//! so a module that handed out platform paths would be a keystroke recorder
//! with extra steps. Everything below is arranged around three consequences of
//! that: the public identity of a device is an opaque per-process token, a
//! Generic Desktop keyboard, keypad, mouse or pointer collection is refused
//! before a handle exists, and the refusal covers the whole physical node
//! rather than the one collection that named the usage.
//!
//! The second arrangement is threading. A HID read blocks, and a blocking read
//! on the thread that paints is a dropped frame; a read that called into
//! JavaScript from the thread it blocked on would be worse. Each open device
//! therefore gets a worker that owns its handle outright — `hidapi`'s device is
//! `Send` and not `Sync`, so ownership rather than sharing is also the only
//! thing the type system allows — and every report, completion and disconnect
//! crosses back as a signal that [`HidController::poll`] turns into a frame-turn
//! message.
//!
//! The third arrangement is Android's, and it is why opening is allowed to take
//! more than one turn (#248). There a device is reached through `UsbManager`
//! and access is a permission a person grants to a system dialog, so a backend
//! may answer "asked, not answered yet" — `Ok(None)` — and the controller keeps
//! the caller's promise open across frames until the answer arrives. Everything
//! after the handle exists is identical on both platforms, which is the point:
//! one worker, one bound check, one terminal event, one set of `DOMException`
//! names.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::dom_bridge::hid::{Failure, Message};

/// Android's `UsbManager` backend, whose logic is compiled and tested here.
///
/// `cfg(test)` puts it in a Linux build as well as an Android one on purpose.
/// None of it is Android-specific except the JNI calls it makes through
/// [`android::UsbApi`], and a permission state machine that only exists on a
/// platform this workspace cannot run tests on would be a permission state
/// machine nothing ever checked.
#[cfg(any(target_os = "android", test))]
pub(crate) mod android;
#[cfg(not(target_os = "android"))]
mod platform;

/// Usages an application may never open, whatever it asks for.
///
/// Generic Desktop keyboard, keypad, mouse and pointer are the four collections
/// the operating system routes ordinary input through. Reading them raw is
/// keylogging and cursor surveillance, which is a different product with a
/// different consent story; DOM events and #94's Gamepad support are what an
/// application uses instead. There is deliberately no opt-out.
const PROTECTED_USAGE_PAGE: u16 = 0x01;
const PROTECTED_USAGES: [u16; 4] = [0x01, 0x02, 0x06, 0x07];

/// The largest report Blitsen will move in either direction, in bytes.
///
/// The per-device bound derived from the report descriptor is the real limit
/// and is almost always far smaller. This is the second check S10 asked for: a
/// device that declares a preposterous report, or one whose descriptor could
/// not be parsed at all, still cannot make the host allocate without bound.
/// 4 KiB is an order of magnitude above the largest report seen in practice.
pub(crate) const MAX_REPORT_BYTES: usize = 4096;

/// How long a device worker blocks in a read before it looks at its mailbox.
///
/// The worker owns the handle, so a write waits for the read in flight. Eight
/// milliseconds is half a 60 Hz frame — shorter than the frame turn that queued
/// the write — and costs 125 poll syscalls a second on a thread that is asleep
/// for all of them.
const READ_POLL: i32 = 8;

/// How often hot-plug is rescanned while an application is listening.
///
/// Enumeration walks the platform's device tree, so it is not free. A second is
/// far below the human timescale of plugging something in and far above the
/// frame rate, and no scan happens at all while nothing is listening.
const HOTPLUG_INTERVAL: Duration = Duration::from_secs(1);

/// One top-level collection as the platform enumerated it.
///
/// `path` is the platform's own device path and never leaves this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BackendDevice {
    pub(crate) path: String,
    pub(crate) vendor_id: u16,
    pub(crate) product_id: u16,
    pub(crate) release_number: u16,
    pub(crate) usage_page: u16,
    pub(crate) usage: u16,
    pub(crate) product_name: Option<String>,
    pub(crate) manufacturer_name: Option<String>,
    pub(crate) serial_number: Option<String>,
}

/// What an opened handle can do, and the seam the tests replace.
///
/// Not `Sync` on purpose: it mirrors `hidapi::HidDevice`, and a trait that
/// promised more than the only real implementor can deliver would push the
/// worker-owns-the-handle decision out of the type system and into a comment.
pub(crate) trait HidHandle: Send {
    /// The raw HID report descriptor, for report bounds and the composite check.
    fn report_descriptor(&self) -> Result<Vec<u8>, String>;
    /// Sends an output report whose first byte is the report ID.
    fn write(&self, data: &[u8]) -> Result<(), String>;
    /// Sends a feature report whose first byte is the report ID.
    fn send_feature_report(&self, data: &[u8]) -> Result<(), String>;
    /// Reads a feature report into a buffer whose first byte is the report ID.
    fn get_feature_report(&self, buffer: &mut [u8]) -> Result<usize, String>;
    /// Reads one input report, answering `0` when `timeout` elapsed first.
    fn read_timeout(&self, buffer: &mut [u8], timeout: i32) -> Result<usize, String>;
}

/// Enumeration and opening, so both can be driven without hardware.
pub(crate) trait HidBackend: Send {
    fn enumerate(&mut self) -> Result<Vec<BackendDevice>, String>;
    /// Opens a device, answering `None` while the platform is still deciding.
    ///
    /// A desktop open either has the access or does not and the answer is the
    /// syscall's. Android asks a person, through a dialog that stays up for as
    /// long as they take, so `None` means "asked, no answer yet" — the caller's
    /// promise is held rather than rejected, and the backend is asked again on
    /// the next frame turn. Rejecting and making the application call `open` a
    /// second time would have made one call mean two different things on two
    /// platforms.
    fn open(&mut self, device: &BackendDevice) -> Result<Option<Box<dyn HidHandle>>, Failure>;
}

/// What a device worker is asked to do between reads.
enum Command {
    Write { command_id: u64, data: Vec<u8> },
    SendFeatureReport { command_id: u64, data: Vec<u8> },
    ReceiveFeatureReport { command_id: u64, report_id: u8 },
}

/// What a device worker reports back.
enum Signal {
    /// An input report, already split into its report ID and payload.
    Input {
        device_id: String,
        report_id: u8,
        data: Vec<u8>,
    },
    /// A queued command settled.
    Completion {
        command_id: u64,
        result: Result<Option<Vec<u8>>, Failure>,
    },
    /// The device stopped answering. At most one per device, ever.
    Disconnected { device_id: String },
}

/// The report bounds and framing a descriptor declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReportLimits {
    input: usize,
    output: usize,
    feature: usize,
    /// Whether input reports carry a leading report-ID byte on the wire.
    input_has_ids: bool,
}

impl Default for ReportLimits {
    fn default() -> Self {
        Self {
            input: MAX_REPORT_BYTES,
            output: MAX_REPORT_BYTES,
            feature: MAX_REPORT_BYTES,
            input_has_ids: true,
        }
    }
}

/// An open the platform has been asked about and has not answered yet.
///
/// The device is remembered rather than looked up again on each retry: a
/// permission dialog is up for as long as a person takes to read it, and
/// re-enumerating the USB tree once a frame for the whole of that would be a
/// device-tree walk per frame to learn something the retry itself reports.
struct PendingOpen {
    target: BackendDevice,
    info: Value,
    /// The one command this will settle.
    command_id: u64,
}

struct OpenDevice {
    commands: Sender<Command>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    limits: ReportLimits,
    /// Set by the first disconnect, so the terminal event is emitted once.
    terminated: bool,
}

/// Everything `blitsen/hid` owns for one application session.
pub(crate) struct HidController {
    backend: Box<dyn HidBackend>,
    wake: Arc<dyn Fn() + Send + Sync>,
    signals: Arc<Mutex<VecDeque<Signal>>>,
    /// Platform path to public id. Every enumerated path is here, including the
    /// protected ones: an id must stay stable, and refusing an id needs to be
    /// distinguishable from never having heard of it.
    ids: HashMap<String, String>,
    next_id: u64,
    /// The last enumeration, keyed by public id, which hot-plug diffs against.
    present: BTreeMap<String, Value>,
    open: HashMap<String, OpenDevice>,
    /// Opens waiting on a permission answer, by public id.
    pending: HashMap<String, PendingOpen>,
    /// Command id to the device that owes it an answer.
    inflight: HashMap<u64, String>,
    last_scan: Option<Instant>,
}

impl Drop for HidController {
    fn drop(&mut self) {
        for id in self.open.keys().cloned().collect::<Vec<_>>() {
            self.shutdown(&id);
        }
    }
}

/// Whether a top-level collection is one an application may never open.
fn protected(usage_page: u16, usage: u16) -> bool {
    usage_page == PROTECTED_USAGE_PAGE && PROTECTED_USAGES.contains(&usage)
}

/// Groups an enumeration by platform path, in the order it arrived.
///
/// This grouping *is* the composite-device rule. On Linux one hidraw node
/// carries every top-level collection of the physical device, so opening the
/// vendor collection of a keyboard with a configuration interface would hand
/// over the keyboard as well. A path with any protected collection is therefore
/// refused whole. Windows and macOS give each collection its own path, where
/// the same rule degenerates to refusing exactly the protected one.
fn grouped(devices: Vec<BackendDevice>) -> Vec<(String, Vec<BackendDevice>)> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<BackendDevice>> = HashMap::new();
    for device in devices {
        if !groups.contains_key(&device.path) {
            order.push(device.path.clone());
        }
        groups.entry(device.path.clone()).or_default().push(device);
    }
    order
        .into_iter()
        .map(|path| {
            let group = groups.remove(&path).expect("path was recorded in order");
            (path, group)
        })
        .collect()
}

/// The public record for one physical device, from its collections.
fn describe(id: &str, group: &[BackendDevice]) -> Value {
    let first = &group[0];
    json!({
        "id": id,
        "vendorId": first.vendor_id,
        "productId": first.product_id,
        "releaseNumber": first.release_number,
        "usagePage": first.usage_page,
        "usage": first.usage,
        "usages": group
            .iter()
            .map(|device| json!({ "usagePage": device.usage_page, "usage": device.usage }))
            .collect::<Vec<_>>(),
        "productName": first.product_name,
        "manufacturerName": first.manufacturer_name,
        // Metadata, never identity (S10). Two devices of the same model are
        // told apart by their `id`, which says nothing about the hardware and
        // means nothing after this process exits.
        "serialNumber": first.serial_number,
    })
}

/// Reads report bounds and top-level collections out of a report descriptor.
///
/// Answers `None` when the descriptor is unreadable or unparseable, which is
/// not fatal: enumeration has already applied the collection filter, and the
/// conservative ceiling still bounds every transfer.
fn limits_of(descriptor: &[u8]) -> Option<(ReportLimits, Vec<(u16, u16)>)> {
    use hidreport::{CollectionType, Report, ReportDescriptor};

    let parsed = ReportDescriptor::try_from(descriptor).ok()?;
    // `size_in_bytes` already counts the leading report-ID byte where the
    // device uses report IDs. `hidapi` wants that byte on output and feature
    // transfers regardless, so a device without report IDs is one byte wider
    // on the wire than it declares.
    fn widest(reports: &[impl Report]) -> Option<usize> {
        reports
            .iter()
            .map(|report| report.size_in_bytes() + usize::from(report.report_id().is_none()))
            .max()
    }
    let input_has_ids = parsed
        .input_reports()
        .first()
        .is_some_and(|report| report.report_id().is_some());
    let limits = ReportLimits {
        input: parsed
            .input_reports()
            .iter()
            .map(Report::size_in_bytes)
            .max()
            .unwrap_or(MAX_REPORT_BYTES)
            .clamp(1, MAX_REPORT_BYTES),
        output: widest(parsed.output_reports())
            .unwrap_or(MAX_REPORT_BYTES)
            .clamp(1, MAX_REPORT_BYTES),
        feature: widest(parsed.feature_reports())
            .unwrap_or(MAX_REPORT_BYTES)
            .clamp(1, MAX_REPORT_BYTES),
        input_has_ids,
    };
    let mut collections = Vec::new();
    for report in parsed
        .input_reports()
        .iter()
        .map(|report| report.fields())
        .chain(parsed.output_reports().iter().map(|report| report.fields()))
        .chain(parsed.feature_reports().iter().map(|report| report.fields()))
    {
        for field in report {
            for collection in field.collections() {
                if collection.collection_type() != CollectionType::Application {
                    continue;
                }
                for usage in collection.usages() {
                    let packed = u32::from(usage);
                    let entry = ((packed >> 16) as u16, (packed & 0xffff) as u16);
                    if !collections.contains(&entry) {
                        collections.push(entry);
                    }
                }
            }
        }
    }
    Some((limits, collections))
}

/// The loop one open device runs on.
///
/// Reads are the reason it exists; commands are drained between them so that
/// exactly one thread ever touches the handle. It never calls into JavaScript
/// and never touches the controller — everything leaves through `signals`.
fn worker(
    device_id: String,
    handle: Box<dyn HidHandle>,
    limits: ReportLimits,
    commands: Receiver<Command>,
    stop: Arc<AtomicBool>,
    signals: Arc<Mutex<VecDeque<Signal>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let emit = |signal| {
        signals.lock().push_back(signal);
        wake();
    };
    let mut buffer = vec![0u8; limits.input];
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Write { command_id, data }) => emit(Signal::Completion {
                    command_id,
                    result: handle
                        .write(&data)
                        .map(|()| None)
                        .map_err(Failure::operation),
                }),
                Ok(Command::SendFeatureReport { command_id, data }) => emit(Signal::Completion {
                    command_id,
                    result: handle
                        .send_feature_report(&data)
                        .map(|()| None)
                        .map_err(Failure::operation),
                }),
                Ok(Command::ReceiveFeatureReport {
                    command_id,
                    report_id,
                }) => {
                    let mut report = vec![0u8; limits.feature];
                    report[0] = report_id;
                    let result = handle
                        .get_feature_report(&mut report)
                        .map(|read| {
                            // The report ID is what the caller asked for, so
                            // the payload it gets back is the report without
                            // it — the same framing as an input report.
                            report.truncate(read.min(limits.feature));
                            Some(report.split_off(usize::from(!report.is_empty())))
                        })
                        .map_err(Failure::operation);
                    emit(Signal::Completion { command_id, result });
                }
                Err(TryRecvError::Empty) => break,
                // The controller dropped the sender, which only happens when
                // the device is being closed; `stop` says so authoritatively.
                Err(TryRecvError::Disconnected) => break,
            }
        }
        if stop.load(Ordering::Acquire) {
            return;
        }
        match handle.read_timeout(&mut buffer, READ_POLL) {
            Ok(0) => {}
            Ok(read) => {
                let report = &buffer[..read.min(buffer.len())];
                let (report_id, payload) = if limits.input_has_ids {
                    (report[0], &report[1..])
                } else {
                    (0, report)
                };
                emit(Signal::Input {
                    device_id: device_id.clone(),
                    report_id,
                    data: payload.to_vec(),
                });
            }
            Err(_) => {
                // A read that fails is a device that went away or a handle the
                // platform revoked. Either way this device is finished, and the
                // controller turns this into the one terminal event.
                emit(Signal::Disconnected {
                    device_id: device_id.clone(),
                });
                return;
            }
        }
    }
}

impl HidController {
    /// Builds a controller over an injected backend and wake-up.
    ///
    /// The wake-up is a closure rather than a winit proxy so that the whole of
    /// this file can be driven from a test with no event loop and no hardware.
    pub(crate) fn with_backend(
        backend: Box<dyn HidBackend>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            backend,
            wake,
            signals: Arc::new(Mutex::new(VecDeque::new())),
            ids: HashMap::new(),
            next_id: 1,
            present: BTreeMap::new(),
            open: HashMap::new(),
            pending: HashMap::new(),
            inflight: HashMap::new(),
            last_scan: None,
        }
    }

    fn public_id(&mut self, path: &str) -> String {
        if let Some(id) = self.ids.get(path) {
            return id.clone();
        }
        let id = format!("d{}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.ids.insert(path.to_owned(), id.clone());
        id
    }

    /// Enumerates, applies the collection filter, and refreshes the registry.
    fn scan(&mut self) -> Result<Scan, Failure> {
        let enumerated = self.backend.enumerate().map_err(Failure::operation)?;
        let mut scan = Scan::default();
        for (path, group) in grouped(enumerated) {
            let id = self.public_id(&path);
            if group
                .iter()
                .any(|device| protected(device.usage_page, device.usage))
            {
                scan.refused.push(id);
                continue;
            }
            let info = describe(&id, &group);
            let first = group.into_iter().next().expect("a group has a member");
            scan.openable.insert(id, (first, info));
        }
        Ok(scan)
    }

    /// Answers the current device snapshot, emitting hot-plug edges it implies.
    pub(crate) fn devices(&mut self) -> Result<Value, Failure> {
        let scan = self.scan()?;
        let snapshot = scan
            .openable
            .values()
            .map(|(_, info)| info.clone())
            .collect::<Vec<_>>();
        self.diff(scan.snapshot());
        Ok(Value::Array(snapshot))
    }

    /// Turns a fresh snapshot into connect and disconnect events.
    fn diff(&mut self, next: BTreeMap<String, Value>) {
        if crate::dom_bridge::hid::watching() {
            for (id, info) in &next {
                if !self.present.contains_key(id) {
                    crate::dom_bridge::hid::push(Message::change("connected", info.clone()));
                }
            }
            for (id, info) in &self.present {
                if !next.contains_key(id) {
                    crate::dom_bridge::hid::push(Message::change("disconnected", info.clone()));
                }
            }
        }
        self.present = next;
    }

    /// Opens a device by public id, refusing everything S10 said to refuse.
    ///
    /// Answers `Ok(None)` when the platform has been asked for access and has
    /// not answered: the command id is held until it does, and the completion
    /// is pushed from [`HidController::poll`] instead of returned here.
    pub(crate) fn open(
        &mut self,
        device_id: &str,
        command_id: u64,
    ) -> Result<Option<Value>, Failure> {
        if self.open.contains_key(device_id) {
            return Err(Failure::invalid_state(format!(
                "HID device {device_id} is already open"
            )));
        }
        // A second open while the first is still waiting is refused with the
        // same name a second open of an open device gets. Two reasons, and they
        // point the same way: an application that did this on a desktop was
        // told `InvalidStateError`, and one permission dialog per device is the
        // most a person should ever be shown for one request.
        if self.pending.contains_key(device_id) {
            return Err(Failure::invalid_state(format!(
                "an open of HID device {device_id} is already waiting for permission"
            )));
        }
        // Re-enumerated rather than read from the last snapshot: a device can
        // be unplugged, or gain a protected collection by being replaced with
        // another one on the same node, between the snapshot and this call.
        let scan = self.scan()?;
        if scan.refused.iter().any(|id| id == device_id) {
            return Err(Failure::not_supported(format!(
                "HID device {device_id} exposes a protected keyboard, keypad, mouse or pointer \
                 collection and cannot be opened"
            )));
        }
        let Some((target, info)) = scan.openable.get(device_id).cloned() else {
            return Err(Failure::not_found(format!(
                "HID device {device_id} is no longer connected"
            )));
        };
        self.diff(scan.snapshot());
        if let Some(opened) = self.start(device_id, &target, &info)? {
            return Ok(Some(opened));
        }
        self.pending.insert(
            device_id.to_owned(),
            PendingOpen {
                target,
                info,
                command_id,
            },
        );
        Ok(None)
    }

    /// Takes a resolved device from the backend's answer to a live handle.
    ///
    /// Split out of [`HidController::open`] because a pending open runs it
    /// again on later turns, and everything it does — the descriptor gate, the
    /// worker, the bounds an application is told — has to be identical whether
    /// permission was already held or was granted a second later.
    fn start(
        &mut self,
        device_id: &str,
        target: &BackendDevice,
        info: &Value,
    ) -> Result<Option<Value>, Failure> {
        let Some(handle) = self.backend.open(target)? else {
            return Ok(None);
        };
        let mut limits = ReportLimits::default();
        if let Ok(descriptor) = handle.report_descriptor()
            && let Some((declared, collections)) = limits_of(&descriptor)
        {
            // The last gate, and the only one that reads the device rather than
            // the platform's index of it. A composite device whose enumeration
            // hid a protected collection is caught here, with the handle closed
            // by the drop below before a single report is read.
            if let Some((usage_page, usage)) = collections
                .iter()
                .copied()
                .find(|(usage_page, usage)| protected(*usage_page, *usage))
            {
                return Err(Failure::not_supported(format!(
                    "HID device {device_id} declares a protected top-level collection \
                     (usage page {usage_page:#06x}, usage {usage:#06x}) and cannot be opened"
                )));
            }
            limits = declared;
        }

        let (sender, receiver) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let signals = Arc::clone(&self.signals);
        let wake = Arc::clone(&self.wake);
        let worker_id = device_id.to_owned();
        let handle = std::thread::Builder::new()
            .name(format!("blitsen-hid-{device_id}"))
            .spawn(move || {
                worker(
                    worker_id, handle, limits, receiver, worker_stop, signals, wake,
                );
            })
            .map_err(|error| Failure::operation(format!("could not start a HID reader: {error}")))?;
        self.open.insert(
            device_id.to_owned(),
            OpenDevice {
                commands: sender,
                stop,
                worker: Some(handle),
                limits,
                terminated: false,
            },
        );
        Ok(Some(json!({
            "device": info,
            "maxInputReportSize": limits.input,
            "maxOutputReportSize": limits.output,
            "maxFeatureReportSize": limits.feature,
        })))
    }

    /// Stops a device's worker, drops its handle, and settles what was in flight.
    ///
    /// A transfer the worker had accepted but not finished dies with it, and a
    /// promise that never settles would keep the frame loop awake forever
    /// waiting for a device that is gone. Every outstanding command is failed
    /// here instead, before the caller learns the device closed.
    fn shutdown(&mut self, device_id: &str) -> bool {
        let Some(mut device) = self.open.remove(device_id) else {
            return false;
        };
        device.stop.store(true, Ordering::Release);
        drop(device.commands);
        if let Some(worker) = device.worker.take() {
            let _ = worker.join();
        }
        let mut abandoned = Vec::new();
        self.inflight.retain(|command_id, owner| {
            if owner == device_id {
                abandoned.push(*command_id);
                return false;
            }
            true
        });
        for command_id in abandoned {
            crate::dom_bridge::hid::complete_bytes(
                command_id,
                Err(Failure::invalid_state(format!(
                    "HID device {device_id} closed before this transfer finished"
                ))),
            );
        }
        true
    }

    /// Closes a device on the application's request.
    ///
    /// Deliberately not a disconnect: an application that asked for this knows
    /// the device is gone, and a terminal event here would be indistinguishable
    /// from the cable being pulled.
    pub(crate) fn close(&mut self, device_id: &str) -> Result<Value, Failure> {
        if !self.shutdown(device_id) {
            return Err(Failure::invalid_state(format!(
                "HID device {device_id} is not open"
            )));
        }
        Ok(Value::Null)
    }

    /// Validates a transfer against the device's declared bound, then queues it.
    ///
    /// The length check happens here, before anything is copied to a worker or
    /// handed to the platform: an oversized report is a mistake in the call and
    /// must not become an allocation the device asked for.
    fn queue(&mut self, device_id: &str, command_id: u64, kind: TransferKind) -> Result<(), Failure> {
        let Some(device) = self.open.get(device_id) else {
            return Err(Failure::invalid_state(format!(
                "HID device {device_id} is not open"
            )));
        };
        let command = match kind {
            TransferKind::Write(data) => {
                check_length(&data, device.limits.output, "output report")?;
                Command::Write { command_id, data }
            }
            TransferKind::Feature(data) => {
                check_length(&data, device.limits.feature, "feature report")?;
                Command::SendFeatureReport { command_id, data }
            }
            TransferKind::ReceiveFeature(report_id) => Command::ReceiveFeatureReport {
                command_id,
                report_id,
            },
        };
        device.commands.send(command).map_err(|_| {
            Failure::invalid_state(format!("HID device {device_id} stopped responding"))
        })?;
        self.inflight.insert(command_id, device_id.to_owned());
        Ok(())
    }

    /// Queues an output report.
    pub(crate) fn write(
        &mut self,
        device_id: &str,
        command_id: u64,
        data: Vec<u8>,
    ) -> Result<(), Failure> {
        self.queue(device_id, command_id, TransferKind::Write(data))
    }

    /// Queues a feature-report write.
    pub(crate) fn send_feature_report(
        &mut self,
        device_id: &str,
        command_id: u64,
        data: Vec<u8>,
    ) -> Result<(), Failure> {
        self.queue(device_id, command_id, TransferKind::Feature(data))
    }

    /// Queues a feature-report read.
    pub(crate) fn receive_feature_report(
        &mut self,
        device_id: &str,
        command_id: u64,
        report_id: u8,
    ) -> Result<(), Failure> {
        self.queue(device_id, command_id, TransferKind::ReceiveFeature(report_id))
    }

    /// Drains worker signals into the frame-turn queue and rescans hot-plug.
    ///
    /// The only place a HID event becomes visible to an application, and the
    /// only place enumeration happens without the application asking: while
    /// nothing is listening for device changes there is no scan at all.
    pub(crate) fn poll(&mut self) {
        let signals = self.signals.lock().drain(..).collect::<Vec<_>>();
        for signal in signals {
            match signal {
                Signal::Input {
                    device_id,
                    report_id,
                    data,
                } => {
                    if self.open.contains_key(&device_id) {
                        crate::dom_bridge::hid::push(Message::input(device_id, report_id, data));
                    }
                }
                Signal::Completion { command_id, result } => {
                    self.inflight.remove(&command_id);
                    crate::dom_bridge::hid::complete_bytes(command_id, result);
                }
                Signal::Disconnected { device_id } => {
                    let Some(device) = self.open.get_mut(&device_id) else {
                        continue;
                    };
                    if device.terminated {
                        continue;
                    }
                    device.terminated = true;
                    self.shutdown(&device_id);
                    crate::dom_bridge::hid::push(Message::disconnect(device_id));
                }
            }
        }
        self.resolve_pending();
        if !crate::dom_bridge::hid::watching() {
            self.last_scan = None;
            return;
        }
        if self
            .last_scan
            .is_some_and(|last| last.elapsed() < HOTPLUG_INTERVAL)
        {
            return;
        }
        self.last_scan = Some(Instant::now());
        if let Ok(scan) = self.scan() {
            self.diff(scan.snapshot());
        }
    }

    /// Asks the backend again about every open that is waiting for permission.
    ///
    /// Every frame turn rather than on the hot-plug interval, because what is
    /// being waited for is a person tapping a dialog and the application's
    /// promise cannot settle before the attempt that observes it. The device is
    /// the one the request named, so nothing is enumerated: waiting costs one
    /// backend call per turn per outstanding request, and a backend that still
    /// has no answer says so without touching the device.
    fn resolve_pending(&mut self) {
        for device_id in self.pending.keys().cloned().collect::<Vec<_>>() {
            let Some(pending) = self.pending.get(&device_id) else {
                continue;
            };
            let (target, info) = (pending.target.clone(), pending.info.clone());
            let settled = match self.start(&device_id, &target, &info) {
                Ok(None) => continue,
                Ok(Some(opened)) => Ok(opened),
                Err(failure) => Err(failure),
            };
            let Some(pending) = self.pending.remove(&device_id) else {
                continue;
            };
            crate::dom_bridge::hid::complete(pending.command_id, settled);
        }
    }
}

enum TransferKind {
    Write(Vec<u8>),
    Feature(Vec<u8>),
    ReceiveFeature(u8),
}

/// One enumeration, split into what may be opened and what was refused.
#[derive(Default)]
struct Scan {
    /// Openable devices by public id, with the entry each one opens through.
    openable: BTreeMap<String, (BackendDevice, Value)>,
    /// Ids the collection filter refused, so `open` can say why rather than
    /// reporting a device it deliberately hid as missing.
    refused: Vec<String>,
}

impl Scan {
    fn snapshot(&self) -> BTreeMap<String, Value> {
        self.openable
            .iter()
            .map(|(id, (_, info))| (id.clone(), info.clone()))
            .collect()
    }
}

fn check_length(data: &[u8], limit: usize, what: &str) -> Result<(), Failure> {
    if data.is_empty() {
        return Err(Failure::data(format!(
            "a HID {what} needs at least the report ID byte"
        )));
    }
    if data.len() > limit {
        return Err(Failure::data(format!(
            "a HID {what} of {} bytes exceeds the {limit} this device declared",
            data.len()
        )));
    }
    Ok(())
}

/// The controller a window session drives, over the platform's own backend.
#[cfg(not(target_os = "android"))]
pub(crate) fn controller(proxy: winit::event_loop::EventLoopProxy) -> HidController {
    HidController::with_backend(
        Box::new(platform::HidApiBackend::default()),
        Arc::new(move || proxy.wake_up()),
    )
}

/// The same controller over `UsbManager`, which is Android's whole HID story.
#[cfg(target_os = "android")]
pub(crate) fn controller(proxy: winit::event_loop::EventLoopProxy) -> HidController {
    HidController::with_backend(
        Box::new(android::UsbHidBackend::new(android::usb::ActivityUsb)),
        Arc::new(move || proxy.wake_up()),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicIsize, AtomicUsize};

    use super::*;

    /// A backend with no hardware behind it.
    ///
    /// `defer` is how a platform that asks a person for permission is driven
    /// from a test: the first `defer` opens answer `Pending`, as Android's does
    /// while its dialog is up, and the one after that produces the handle.
    #[derive(Default)]
    struct FakeBackend {
        devices: Arc<Mutex<Vec<BackendDevice>>>,
        handles: Arc<Mutex<Vec<FakeState>>>,
        refuse: Option<Failure>,
        unavailable: Option<String>,
        defer: Arc<AtomicIsize>,
    }

    /// Input reports the fake handle answers, then a read error to end on.
    type Reports = Arc<Mutex<VecDeque<Result<Vec<u8>, String>>>>;

    #[derive(Clone, Default)]
    struct FakeState {
        descriptor: Vec<u8>,
        reports: Reports,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        feature: Arc<Mutex<Vec<u8>>>,
    }

    struct FakeHandle(FakeState);

    impl HidHandle for FakeHandle {
        fn report_descriptor(&self) -> Result<Vec<u8>, String> {
            Ok(self.0.descriptor.clone())
        }

        fn write(&self, data: &[u8]) -> Result<(), String> {
            self.0.written.lock().push(data.to_vec());
            Ok(())
        }

        fn send_feature_report(&self, data: &[u8]) -> Result<(), String> {
            *self.0.feature.lock() = data.to_vec();
            Ok(())
        }

        fn get_feature_report(&self, buffer: &mut [u8]) -> Result<usize, String> {
            let stored = self.0.feature.lock().clone();
            let len = stored.len().min(buffer.len());
            buffer[..len].copy_from_slice(&stored[..len]);
            Ok(len)
        }

        fn read_timeout(&self, buffer: &mut [u8], timeout: i32) -> Result<usize, String> {
            let next = self.0.reports.lock().pop_front();
            match next {
                Some(Ok(report)) => {
                    let len = report.len().min(buffer.len());
                    buffer[..len].copy_from_slice(&report[..len]);
                    Ok(len)
                }
                Some(Err(error)) => Err(error),
                None => {
                    std::thread::sleep(Duration::from_millis(timeout.max(1) as u64));
                    Ok(0)
                }
            }
        }
    }

    impl HidBackend for FakeBackend {
        fn enumerate(&mut self) -> Result<Vec<BackendDevice>, String> {
            if let Some(error) = &self.unavailable {
                return Err(error.clone());
            }
            Ok(self.devices.lock().clone())
        }

        fn open(&mut self, device: &BackendDevice) -> Result<Option<Box<dyn HidHandle>>, Failure> {
            if let Some(failure) = &self.refuse {
                return Err(failure.clone());
            }
            let _ = device;
            if self.defer.fetch_sub(1, Ordering::AcqRel) > 0 {
                return Ok(None);
            }
            let state = self.handles.lock().first().cloned().unwrap_or_default();
            Ok(Some(Box::new(FakeHandle(state))))
        }
    }

    fn device(path: &str, usage_page: u16, usage: u16) -> BackendDevice {
        BackendDevice {
            path: path.into(),
            vendor_id: 0x1234,
            product_id: 0x5678,
            release_number: 0x0100,
            usage_page,
            usage,
            product_name: Some("Widget".into()),
            manufacturer_name: Some("Acme".into()),
            serial_number: Some("SN-9".into()),
        }
    }

    fn controller_over(backend: FakeBackend) -> (HidController, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        let controller = HidController::with_backend(
            Box::new(backend),
            Arc::new(move || {
                counter.fetch_add(1, Ordering::Release);
            }),
        );
        (controller, wakes)
    }

    /// Waits for the worker thread to deliver, without a fixed sleep.
    fn settle(controller: &mut HidController, wanted: usize) -> Vec<Message> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut messages = Vec::new();
        while Instant::now() < deadline {
            controller.poll();
            messages.extend(crate::dom_bridge::hid::take_messages());
            if messages.len() >= wanted {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        messages
    }

    /// A minimal vendor-defined descriptor: one 4-byte input report with ID 3,
    /// one 2-byte output report and one 2-byte feature report, all under a
    /// vendor application collection.
    fn vendor_descriptor() -> Vec<u8> {
        vec![
            0x06, 0x00, 0xff, // Usage Page (Vendor Defined 0xff00)
            0x09, 0x01, // Usage (0x01)
            0xa1, 0x01, // Collection (Application)
            0x85, 0x03, //   Report ID (3)
            0x09, 0x02, //   Usage (0x02)
            0x75, 0x08, //   Report Size (8)
            0x95, 0x03, //   Report Count (3)
            0x81, 0x02, //   Input (Data, Var, Abs)
            0x09, 0x03, //   Usage (0x03)
            0x95, 0x01, //   Report Count (1)
            0x91, 0x02, //   Output (Data, Var, Abs)
            0x09, 0x04, //   Usage (0x04)
            0xb1, 0x02, //   Feature (Data, Var, Abs)
            0xc0, // End Collection
        ]
    }

    /// The same device with a Generic Desktop keyboard collection bolted on,
    /// which enumeration did not mention.
    fn composite_descriptor() -> Vec<u8> {
        let mut descriptor = vendor_descriptor();
        descriptor.extend_from_slice(&[
            0x05, 0x01, // Usage Page (Generic Desktop)
            0x09, 0x06, // Usage (Keyboard)
            0xa1, 0x01, // Collection (Application)
            0x85, 0x04, //   Report ID (4)
            0x05, 0x07, //   Usage Page (Keyboard)
            0x19, 0x00, //   Usage Minimum (0)
            0x29, 0x65, //   Usage Maximum (101)
            0x15, 0x00, //   Logical Minimum (0)
            0x25, 0x65, //   Logical Maximum (101)
            0x75, 0x08, //   Report Size (8)
            0x95, 0x06, //   Report Count (6)
            0x81, 0x00, //   Input (Data, Array, Abs)
            0xc0, // End Collection
        ]);
        descriptor
    }

    #[test]
    fn enumeration_hides_protected_collections_and_the_nodes_that_carry_them() {
        crate::dom_bridge::hid::reset();
        let backend = FakeBackend {
            devices: Arc::new(Mutex::new(vec![
                device("/dev/hidraw0", 0xff00, 0x0001),
                // One node, two collections: the vendor one must not become a
                // way to read the keyboard beside it.
                device("/dev/hidraw1", 0xff00, 0x0001),
                device("/dev/hidraw1", 0x0001, 0x0006),
                device("/dev/hidraw2", 0x0001, 0x0002),
                device("/dev/hidraw3", 0x0001, 0x0080),
            ])),
            ..FakeBackend::default()
        };
        let (mut controller, _) = controller_over(backend);
        let devices = controller.devices().expect("enumeration succeeds");
        let ids = devices
            .as_array()
            .expect("devices answers an array")
            .iter()
            .map(|device| device["id"].as_str().expect("an id").to_owned())
            .collect::<Vec<_>>();
        // hidraw0 is vendor-defined and hidraw3 is a Generic Desktop system
        // control, which is not one of the four protected usages.
        assert_eq!(ids, vec!["d1", "d4"]);
    }

    #[test]
    fn device_ids_are_stable_and_carry_no_platform_identity() {
        crate::dom_bridge::hid::reset();
        let devices = Arc::new(Mutex::new(vec![
            device("/dev/hidraw0", 0xff00, 0x0001),
            device("/dev/hidraw1", 0xff00, 0x0002),
        ]));
        let (mut controller, _) = controller_over(FakeBackend {
            devices: Arc::clone(&devices),
            ..FakeBackend::default()
        });
        let first = controller.devices().expect("enumeration succeeds");
        // The first device goes away and comes back; the second keeps its id
        // throughout, and the first never reuses the id of anything else.
        devices.lock().remove(0);
        controller.devices().expect("enumeration succeeds");
        devices.lock().push(device("/dev/hidraw0", 0xff00, 0x0001));
        let third = controller.devices().expect("enumeration succeeds");
        assert_eq!((&first[0]["id"], &first[0]["usage"]), (&json!("d1"), &json!(1)));
        assert_eq!((&third[0]["id"], &third[0]["usage"]), (&json!("d1"), &json!(1)));
        assert_eq!((&third[1]["id"], &third[1]["usage"]), (&json!("d2"), &json!(2)));
        let rendered = serde_json::to_string(&third).expect("the snapshot serializes");
        assert!(!rendered.contains("hidraw"), "{rendered} names a device path");
    }

    #[test]
    fn open_distinguishes_every_way_it_can_fail() {
        crate::dom_bridge::hid::reset();
        let devices = Arc::new(Mutex::new(vec![
            device("/dev/hidraw0", 0xff00, 0x0001),
            device("/dev/hidraw1", 0xff00, 0x0001),
            device("/dev/hidraw1", 0x0001, 0x0006),
        ]));
        let (mut controller, _) = controller_over(FakeBackend {
            devices: Arc::clone(&devices),
            handles: Arc::new(Mutex::new(vec![FakeState {
                descriptor: vendor_descriptor(),
                ..FakeState::default()
            }])),
            ..FakeBackend::default()
        });
        controller.devices().expect("enumeration succeeds");
        assert_eq!(
            controller.open("d2", 1).expect_err("the keyboard node").name,
            "NotSupportedError"
        );
        assert_eq!(
            controller.open("d9", 2).expect_err("no such device").name,
            "NotFoundError"
        );

        let denied = FakeBackend {
            devices: Arc::clone(&devices),
            refuse: Some(Failure::not_allowed("no udev rule grants access".into())),
            ..FakeBackend::default()
        };
        let (mut controller, _) = controller_over(denied);
        controller.devices().expect("enumeration succeeds");
        assert_eq!(
            controller.open("d1", 1).expect_err("permission denied").name,
            "NotAllowedError"
        );

        // A device that was there when the snapshot was taken and is gone by the
        // time it is opened. Not the same as an id nobody ever issued, and the
        // application has to be able to tell them from a refusal.
        let (mut controller, _) = controller_over(FakeBackend {
            devices: Arc::new(Mutex::new(vec![device("/dev/hidraw0", 0xff00, 0x0001)])),
            ..FakeBackend::default()
        });
        controller.devices().expect("enumeration succeeds");
        controller.backend = Box::new(FakeBackend::default());
        assert_eq!(
            controller.open("d1", 1).expect_err("unplugged since").name,
            "NotFoundError"
        );

        let broken = FakeBackend {
            unavailable: Some("HID support is not installed".into()),
            ..FakeBackend::default()
        };
        let (mut controller, _) = controller_over(broken);
        assert_eq!(
            controller.open("d1", 1).expect_err("the backend itself failed").name,
            "OperationError"
        );
    }

    #[test]
    fn a_composite_device_cannot_smuggle_a_keyboard_past_enumeration() {
        crate::dom_bridge::hid::reset();
        let (mut controller, _) = controller_over(FakeBackend {
            devices: Arc::new(Mutex::new(vec![device("/dev/hidraw0", 0xff00, 0x0001)])),
            handles: Arc::new(Mutex::new(vec![FakeState {
                descriptor: composite_descriptor(),
                ..FakeState::default()
            }])),
            ..FakeBackend::default()
        });
        controller.devices().expect("enumeration succeeds");
        let failure = controller
            .open("d1", 1)
            .expect_err("the descriptor betrays it");
        assert_eq!(failure.name, "NotSupportedError");
        assert!(
            failure.message.contains("0x0001"),
            "{} does not name the collection",
            failure.message
        );
    }

    #[test]
    fn input_reports_keep_their_order_and_report_ids() {
        crate::dom_bridge::hid::reset();
        let reports = Arc::new(Mutex::new(VecDeque::from(vec![
            Ok(vec![0x03, 0x01, 0x02, 0x03]),
            Ok(vec![0x03, 0x04, 0x05, 0x06]),
            Ok(vec![0x03, 0x07, 0x08, 0x09]),
        ])));
        let (mut controller, wakes) = controller_over(FakeBackend {
            devices: Arc::new(Mutex::new(vec![device("/dev/hidraw0", 0xff00, 0x0001)])),
            handles: Arc::new(Mutex::new(vec![FakeState {
                descriptor: vendor_descriptor(),
                reports: Arc::clone(&reports),
                ..FakeState::default()
            }])),
            ..FakeBackend::default()
        });
        controller.devices().expect("enumeration succeeds");
        let opened = controller
            .open("d1", 1)
            .expect("the vendor device opens")
            .expect("a desktop open settles on the turn that asked");
        assert_eq!(opened["maxInputReportSize"], 4);
        assert_eq!(opened["maxOutputReportSize"], 2);
        let delivered = settle(&mut controller, 3);
        assert_eq!(
            delivered
                .iter()
                .map(|message| (message.value.clone(), message.data.clone()))
                .collect::<Vec<_>>(),
            vec![
                (json!({"type":"input","deviceId":"d1","reportId":3}), Some(vec![1, 2, 3])),
                (json!({"type":"input","deviceId":"d1","reportId":3}), Some(vec![4, 5, 6])),
                (json!({"type":"input","deviceId":"d1","reportId":3}), Some(vec![7, 8, 9])),
            ]
        );
        assert!(wakes.load(Ordering::Acquire) >= 3, "the worker woke the loop");
        controller.close("d1").expect("the device closes");
    }

    #[test]
    fn oversized_transfers_are_refused_before_the_device_sees_them() {
        crate::dom_bridge::hid::reset();
        let written = Arc::new(Mutex::new(Vec::new()));
        let (mut controller, _) = controller_over(FakeBackend {
            devices: Arc::new(Mutex::new(vec![device("/dev/hidraw0", 0xff00, 0x0001)])),
            handles: Arc::new(Mutex::new(vec![FakeState {
                descriptor: vendor_descriptor(),
                written: Arc::clone(&written),
                ..FakeState::default()
            }])),
            ..FakeBackend::default()
        });
        controller.devices().expect("enumeration succeeds");
        controller
            .open("d1", 9)
            .expect("the vendor device opens");
        let refused = controller
            .write("d1", 1, vec![0x03; 64])
            .expect_err("64 bytes is past the declared output report");
        assert_eq!(refused.name, "DataError");
        assert!(controller.write("d1", 2, vec![0x03, 0x42]).is_ok());
        assert_eq!(
            controller
                .send_feature_report("d1", 3, vec![0x03; 64])
                .expect_err("the same bound applies to feature reports")
                .name,
            "DataError"
        );
        let settled = settle(&mut controller, 1);
        assert_eq!(
            settled[0].value,
            json!({"type":"completion","commandId":2,"error":null,"errorName":null,"value":null})
        );
        assert_eq!(
            *written.lock(),
            vec![vec![0x03, 0x42]],
            "only the report inside the bound reached the device"
        );
        controller.close("d1").expect("the device closes");
    }

    #[test]
    fn feature_reports_answer_without_the_report_id_they_were_asked_for() {
        crate::dom_bridge::hid::reset();
        let (mut controller, _) = controller_over(FakeBackend {
            devices: Arc::new(Mutex::new(vec![device("/dev/hidraw0", 0xff00, 0x0001)])),
            handles: Arc::new(Mutex::new(vec![FakeState {
                descriptor: vendor_descriptor(),
                ..FakeState::default()
            }])),
            ..FakeBackend::default()
        });
        controller.devices().expect("enumeration succeeds");
        controller
            .open("d1", 9)
            .expect("the vendor device opens");
        controller
            .send_feature_report("d1", 1, vec![0x03, 0x7f])
            .expect("the feature report is within bounds");
        controller
            .receive_feature_report("d1", 2, 0x03)
            .expect("the read is queued");
        let settled = settle(&mut controller, 2);
        assert_eq!(settled[1].data, Some(vec![0x7f]));
        controller.close("d1").expect("the device closes");
    }

    #[test]
    fn a_disconnect_closes_the_handle_and_is_reported_exactly_once() {
        crate::dom_bridge::hid::reset();
        let (mut controller, _) = controller_over(FakeBackend {
            devices: Arc::new(Mutex::new(vec![device("/dev/hidraw0", 0xff00, 0x0001)])),
            handles: Arc::new(Mutex::new(vec![FakeState {
                descriptor: vendor_descriptor(),
                reports: Arc::new(Mutex::new(VecDeque::from(vec![
                    Ok(vec![0x03, 0x01, 0x02, 0x03]),
                    Err("read error".into()),
                ]))),
                ..FakeState::default()
            }])),
            ..FakeBackend::default()
        });
        controller.devices().expect("enumeration succeeds");
        controller
            .open("d1", 9)
            .expect("the vendor device opens");
        let delivered = settle(&mut controller, 2);
        assert_eq!(delivered.len(), 2);
        assert_eq!(delivered[1].value, json!({"type":"disconnect","deviceId":"d1"}));
        // Poll again: the worker is gone, the entry is gone, and no second
        // terminal event can be produced.
        controller.poll();
        assert!(crate::dom_bridge::hid::take_messages().is_empty());
        assert_eq!(
            controller.close("d1").expect_err("the handle is closed").name,
            "InvalidStateError"
        );
    }

    /// The Android open, driven entirely through the shared controller (#248).
    ///
    /// Nothing here knows what a `UsbManager` is: a backend that answers "asked,
    /// no answer yet" is the whole of the platform difference, and what this
    /// asserts is the part an application can observe — the promise stays open
    /// across frames, several opens of one device settle together on the one
    /// answer, and the answer arrives as a completion on a frame turn rather
    /// than from the call that asked.
    #[test]
    fn an_open_awaiting_permission_settles_on_a_later_frame_turn() {
        crate::dom_bridge::hid::reset();
        let (mut controller, _) = controller_over(FakeBackend {
            devices: Arc::new(Mutex::new(vec![device("usb/001/002#0", 0xff00, 0x0001)])),
            handles: Arc::new(Mutex::new(vec![FakeState {
                descriptor: vendor_descriptor(),
                ..FakeState::default()
            }])),
            defer: Arc::new(AtomicIsize::new(2)),
            ..FakeBackend::default()
        });
        controller.devices().expect("enumeration succeeds");
        assert!(
            controller.open("d1", 1).expect("the dialog is up").is_none(),
            "an open with no answer yet does not settle in the call that made it"
        );
        assert_eq!(
            controller
                .open("d1", 2)
                .expect_err("one device, one dialog, one answer")
                .name,
            "InvalidStateError",
            "a second open while the first waits is refused as it is on desktop"
        );
        controller.poll();
        assert!(
            crate::dom_bridge::hid::take_messages().is_empty(),
            "a turn with no answer produces no completion"
        );

        controller.poll();
        let settled = crate::dom_bridge::hid::take_messages();
        assert_eq!(settled.len(), 1, "the open settles once, on a later turn");
        assert_eq!(settled[0].value["type"], "completion");
        assert_eq!(settled[0].value["commandId"], 1);
        assert_eq!(settled[0].value["error"], Value::Null);
        assert_eq!(settled[0].value["value"]["device"]["id"], "d1");
        assert_eq!(settled[0].value["value"]["maxInputReportSize"], 4);
        controller.close("d1").expect("the device closes");
    }

    /// A refusal that arrives after the wait, which is what a denial is.
    #[test]
    fn a_refusal_after_the_wait_rejects_the_open_that_was_waiting() {
        crate::dom_bridge::hid::reset();
        let devices = Arc::new(Mutex::new(vec![device("usb/001/002#0", 0xff00, 0x0001)]));
        let (mut controller, _) = controller_over(FakeBackend {
            devices: Arc::clone(&devices),
            defer: Arc::new(AtomicIsize::new(1)),
            ..FakeBackend::default()
        });
        controller.devices().expect("enumeration succeeds");
        assert!(controller.open("d1", 1).expect("the dialog is up").is_none());
        controller.backend = Box::new(FakeBackend {
            devices,
            refuse: Some(Failure::not_allowed("the user dismissed the dialog".into())),
            ..FakeBackend::default()
        });
        controller.poll();
        let settled = crate::dom_bridge::hid::take_messages();
        assert_eq!(settled.len(), 1);
        // The name, not the text: a denial has to be separable from a device
        // that vanished while the dialog was up, which is a NotFoundError.
        assert_eq!(settled[0].value["errorName"], "NotAllowedError");
        assert_eq!(
            controller
                .close("d1")
                .expect_err("a refused open left nothing open")
                .name,
            "InvalidStateError"
        );
        // And the refusal is not remembered: asking again asks the platform.
        assert!(controller.open("d1", 2).is_err());
    }

    #[test]
    fn hot_plug_is_silent_until_something_is_listening() {
        crate::dom_bridge::hid::reset();
        let devices = Arc::new(Mutex::new(vec![device("/dev/hidraw0", 0xff00, 0x0001)]));
        let (mut controller, _) = controller_over(FakeBackend {
            devices: Arc::clone(&devices),
            ..FakeBackend::default()
        });
        controller.devices().expect("enumeration succeeds");
        devices.lock().push(device("/dev/hidraw1", 0xff00, 0x0002));
        controller.poll();
        assert!(
            crate::dom_bridge::hid::take_messages().is_empty(),
            "nothing listens, so nothing is scanned"
        );

        crate::dom_bridge::hid::watch(true);
        controller.poll();
        let connected = crate::dom_bridge::hid::take_messages();
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].value["type"], "change");
        assert_eq!(connected[0].value["change"], "connected");
        assert_eq!(connected[0].value["device"]["id"], "d2");

        devices.lock().pop();
        controller.last_scan = None;
        controller.poll();
        let disconnected = crate::dom_bridge::hid::take_messages();
        assert_eq!(disconnected.len(), 1);
        assert_eq!(disconnected[0].value["change"], "disconnected");
        assert_eq!(disconnected[0].value["device"]["id"], "d2");
        crate::dom_bridge::hid::watch(false);
    }
}

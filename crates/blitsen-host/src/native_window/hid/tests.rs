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
    assert_eq!(
        (&first[0]["id"], &first[0]["usage"]),
        (&json!("d1"), &json!(1))
    );
    assert_eq!(
        (&third[0]["id"], &third[0]["usage"]),
        (&json!("d1"), &json!(1))
    );
    assert_eq!(
        (&third[1]["id"], &third[1]["usage"]),
        (&json!("d2"), &json!(2))
    );
    let rendered = serde_json::to_string(&third).expect("the snapshot serializes");
    assert!(
        !rendered.contains("hidraw"),
        "{rendered} names a device path"
    );
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
        controller
            .open("d2", 1)
            .expect_err("the keyboard node")
            .name,
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
        controller
            .open("d1", 1)
            .expect_err("permission denied")
            .name,
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
        controller
            .open("d1", 1)
            .expect_err("the backend itself failed")
            .name,
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
            (
                json!({"type":"input","deviceId":"d1","reportId":3}),
                Some(vec![1, 2, 3])
            ),
            (
                json!({"type":"input","deviceId":"d1","reportId":3}),
                Some(vec![4, 5, 6])
            ),
            (
                json!({"type":"input","deviceId":"d1","reportId":3}),
                Some(vec![7, 8, 9])
            ),
        ]
    );
    assert!(
        wakes.load(Ordering::Acquire) >= 3,
        "the worker woke the loop"
    );
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
    controller.open("d1", 9).expect("the vendor device opens");
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
    controller.open("d1", 9).expect("the vendor device opens");
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
    controller.open("d1", 9).expect("the vendor device opens");
    let delivered = settle(&mut controller, 2);
    assert_eq!(delivered.len(), 2);
    assert_eq!(
        delivered[1].value,
        json!({"type":"disconnect","deviceId":"d1"})
    );
    // Poll again: the worker is gone, the entry is gone, and no second
    // terminal event can be produced.
    controller.poll();
    assert!(crate::dom_bridge::hid::take_messages().is_empty());
    assert_eq!(
        controller
            .close("d1")
            .expect_err("the handle is closed")
            .name,
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
        controller
            .open("d1", 1)
            .expect("the dialog is up")
            .is_none(),
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
    assert!(
        controller
            .open("d1", 1)
            .expect("the dialog is up")
            .is_none()
    );
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

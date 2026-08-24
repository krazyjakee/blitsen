//! Main-thread handoff for `blitsen/hid` commands, reports and hot-plug.
//!
//! A HID device reads on a worker thread of its own, and that thread never
//! enters JavaScript. It hands the controller a signal, the controller turns
//! that into one of the messages below, and the bootstrap drains them at the
//! top of a frame — the same FIFO route `blitsen/tray` and `blitsen/notify`
//! take, for the same reason: an application must not be re-entered from a
//! device callback part-way through a turn.
//!
//! A message carries its structured fields as JSON and its report payload as
//! raw bytes beside them. Input reports are the reason: base64 or an array of
//! numbers would re-encode every byte of every report of every frame, and the
//! bridge can already hand a `Uint8Array` straight across.

use std::cell::Cell;

use serde_json::{Value, json};

use super::command_channel::{CommandChannel, CommandRequest};

/// A HID failure, with the `DOMException` name that tells it from the others.
///
/// The four open outcomes S10 required — permission denied, the device gone,
/// a collection Blitsen will not open, and the backend failing — have to be
/// distinguishable by an application without matching on message text, so each
/// one carries the web-platform name that already means it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) name: &'static str,
    pub(crate) message: String,
}

impl Failure {
    /// The platform refused access to a device that is present.
    pub(crate) fn not_allowed(message: String) -> Self {
        Self {
            name: "NotAllowedError",
            message,
        }
    }

    /// No such device, or it disappeared between the snapshot and the call.
    pub(crate) fn not_found(message: String) -> Self {
        Self {
            name: "NotFoundError",
            message,
        }
    }

    /// A collection Blitsen refuses to open, whatever the platform would allow.
    pub(crate) fn not_supported(message: String) -> Self {
        Self {
            name: "NotSupportedError",
            message,
        }
    }

    /// The call is fine but the device is not in a state that accepts it.
    pub(crate) fn invalid_state(message: String) -> Self {
        Self {
            name: "InvalidStateError",
            message,
        }
    }

    /// The report the caller supplied is not one this device can carry.
    pub(crate) fn data(message: String) -> Self {
        Self {
            name: "DataError",
            message,
        }
    }

    /// The backend failed for a reason none of the above describes.
    pub(crate) fn operation(message: String) -> Self {
        Self {
            name: "OperationError",
            message,
        }
    }
}

pub(crate) enum RequestKind {
    Devices,
    Open { device_id: String },
    Close { device_id: String },
    Write { device_id: String, data: Vec<u8> },
    SendFeatureReport { device_id: String, data: Vec<u8> },
    ReceiveFeatureReport { device_id: String, report_id: u8 },
}

pub(crate) type Request = CommandRequest<RequestKind>;

/// One frame-turn message: structured fields, and a report payload beside them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) value: Value,
    pub(crate) data: Option<Vec<u8>>,
}

impl Message {
    fn completion(
        command_id: u64,
        value: Value,
        data: Option<Vec<u8>>,
        error: Option<Failure>,
    ) -> Self {
        let (message, name) = match error {
            Some(failure) => (json!(failure.message), json!(failure.name)),
            None => (Value::Null, Value::Null),
        };
        Self {
            value: json!({
                "type": "completion",
                "commandId": command_id,
                "value": value,
                "error": message,
                "errorName": name,
            }),
            data,
        }
    }

    /// An input report, already split into the report ID and the bytes after it.
    pub(crate) fn input(device_id: String, report_id: u8, data: Vec<u8>) -> Self {
        Self {
            value: json!({ "type": "input", "deviceId": device_id, "reportId": report_id }),
            data: Some(data),
        }
    }

    /// The one terminal event a device produces when it goes away.
    pub(crate) fn disconnect(device_id: String) -> Self {
        Self {
            value: json!({ "type": "disconnect", "deviceId": device_id }),
            data: None,
        }
    }

    /// A hot-plug edge.
    pub(crate) fn change(change: &'static str, device: Value) -> Self {
        Self {
            value: json!({ "type": "change", "change": change, "device": device }),
            data: None,
        }
    }
}

thread_local! {
    static CHANNEL: CommandChannel<RequestKind, Message> = const { CommandChannel::new() };
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

fn request(kind: RequestKind) -> u64 {
    CHANNEL.with(|channel| channel.request(kind))
}

pub(crate) fn devices() -> u64 {
    request(RequestKind::Devices)
}

pub(crate) fn open(device_id: String) -> u64 {
    request(RequestKind::Open { device_id })
}

pub(crate) fn close(device_id: String) -> u64 {
    request(RequestKind::Close { device_id })
}

pub(crate) fn write(device_id: String, data: Vec<u8>) -> u64 {
    request(RequestKind::Write { device_id, data })
}

pub(crate) fn send_feature_report(device_id: String, data: Vec<u8>) -> u64 {
    request(RequestKind::SendFeatureReport { device_id, data })
}

pub(crate) fn receive_feature_report(device_id: String, report_id: u8) -> u64 {
    request(RequestKind::ReceiveFeatureReport {
        device_id,
        report_id,
    })
}

pub(crate) fn take_requests() -> Vec<Request> {
    CHANNEL.with(CommandChannel::take_requests)
}

pub(crate) fn push(message: Message) {
    CHANNEL.with(|channel| channel.push(message));
}

/// Settles a command that answers a structured value.
pub(crate) fn complete(command_id: u64, result: Result<Value, Failure>) {
    let (value, error) = match result {
        Ok(value) => (value, None),
        Err(failure) => (Value::Null, Some(failure)),
    };
    push(Message::completion(command_id, value, None, error));
}

/// Settles a command that answers report bytes, or nothing at all.
pub(crate) fn complete_bytes(command_id: u64, result: Result<Option<Vec<u8>>, Failure>) {
    let (data, error) = match result {
        Ok(data) => (data, None),
        Err(failure) => (None, Some(failure)),
    };
    push(Message::completion(command_id, Value::Null, data, error));
}

/// Whether an application is listening for hot-plug.
///
/// The controller scans only while this is true, which is what keeps an
/// application that never asked about devices from paying for a device tree
/// walk once a second forever.
pub(crate) fn watching() -> bool {
    WATCHING.get()
}

pub(crate) fn watch(watching: bool) {
    WATCHING.set(watching);
}

pub(crate) fn pending() -> bool {
    CHANNEL.with(CommandChannel::pending)
}

pub(crate) fn take_messages() -> Vec<Message> {
    CHANNEL.with(CommandChannel::take_messages)
}

pub(crate) fn reset() {
    WATCHING.set(false);
    CHANNEL.with(CommandChannel::reset);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_reports_and_hot_plug_keep_their_public_fifo_shape() {
        reset();
        complete(1, Ok(json!([])));
        complete_bytes(2, Ok(None));
        complete_bytes(3, Ok(Some(vec![7, 8])));
        complete(4, Err(Failure::not_allowed("no udev rule".into())));
        push(Message::input("d1".into(), 3, vec![1, 2]));
        push(Message::change("connected", json!({ "id": "d2" })));
        push(Message::disconnect("d1".into()));
        let messages = take_messages();
        assert_eq!(
            messages
                .iter()
                .map(|message| message.value.clone())
                .collect::<Vec<_>>(),
            vec![
                json!({"type":"completion","commandId":1,"value":[],"error":null,"errorName":null}),
                json!({"type":"completion","commandId":2,"value":null,"error":null,"errorName":null}),
                json!({"type":"completion","commandId":3,"value":null,"error":null,"errorName":null}),
                json!({"type":"completion","commandId":4,"value":null,"error":"no udev rule",
                    "errorName":"NotAllowedError"}),
                json!({"type":"input","deviceId":"d1","reportId":3}),
                json!({"type":"change","change":"connected","device":{"id":"d2"}}),
                json!({"type":"disconnect","deviceId":"d1"}),
            ]
        );
        assert_eq!(
            messages
                .iter()
                .map(|message| message.data.clone())
                .collect::<Vec<_>>(),
            vec![
                None,
                None,
                Some(vec![7, 8]),
                None,
                Some(vec![1, 2]),
                None,
                None
            ]
        );
        assert!(!pending());
    }
}

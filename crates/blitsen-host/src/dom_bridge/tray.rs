//! Main-thread handoff between the `blitsen/tray` bridge and the window session.
//!
//! JavaScript runs while winit already has the application borrowed. A tray
//! cannot therefore be replaced from the native function that receives the
//! call. Requests are queued here and applied immediately after that pump turn;
//! completions and native tray events travel back through the same queue and
//! are delivered at the top of a later frame.

use serde::Serialize;

use crate::native_window::tray::TraySpec;

use super::command_channel::{CommandChannel, CommandRequest};

pub(crate) enum RequestKind {
    Configure(TraySpec),
    Remove,
}

pub(crate) type Request = CommandRequest<RequestKind>;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum MessageKind {
    Completion {
        #[serde(rename = "commandId")]
        id: u64,
        error: Option<String>,
    },
    Click,
    Action {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        checked: Option<bool>,
    },
}

#[derive(Serialize)]
struct Message {
    #[serde(flatten)]
    kind: MessageKind,
}

thread_local! {
    static CHANNEL: CommandChannel<RequestKind, Message> = const { CommandChannel::new() };
}

fn request(kind: RequestKind) -> u64 {
    CHANNEL.with(|channel| channel.request(kind))
}

pub(crate) fn configure(spec: TraySpec) -> u64 {
    request(RequestKind::Configure(spec))
}

pub(crate) fn remove() -> u64 {
    request(RequestKind::Remove)
}

pub(crate) fn take_requests() -> Vec<Request> {
    CHANNEL.with(CommandChannel::take_requests)
}

pub(crate) fn complete(id: u64, result: Result<(), String>) {
    CHANNEL.with(|channel| {
        channel.push(Message {
            kind: MessageKind::Completion {
                id,
                error: result.err(),
            },
        });
    });
}

pub(crate) fn clicked() {
    CHANNEL.with(|channel| {
        channel.push(Message {
            kind: MessageKind::Click,
        });
    });
}

pub(crate) fn action(id: String, checked: Option<bool>) {
    CHANNEL.with(|channel| {
        channel.push(Message {
            kind: MessageKind::Action { id, checked },
        });
    });
}

pub(crate) fn pending() -> bool {
    CHANNEL.with(CommandChannel::pending)
}

pub(crate) fn take_messages() -> Vec<serde_json::Value> {
    CHANNEL.with(|channel| {
        channel
            .take_messages()
            .into_iter()
            .map(|message| serde_json::to_value(message).expect("tray messages serialize"))
            .collect()
    })
}

pub(crate) fn reset() {
    CHANNEL.with(CommandChannel::reset);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_and_native_events_keep_their_public_fifo_shape() {
        reset();
        complete(7, Ok(()));
        clicked();
        action("open".into(), None);
        action("dark".into(), Some(true));
        assert_eq!(
            take_messages(),
            vec![
                serde_json::json!({ "type": "completion", "commandId": 7, "error": null }),
                serde_json::json!({ "type": "click" }),
                serde_json::json!({ "type": "action", "id": "open" }),
                serde_json::json!({ "type": "action", "id": "dark", "checked": true }),
            ]
        );
    }
}

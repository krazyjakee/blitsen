//! Main-thread handoff between the `blitsen/menu` bridge and the window session.
//!
//! The same shape as `dom_bridge::tray`, and separate from it for the reason
//! the two modules are separate at all: an application menu has no tray to be
//! part of, and a menu the tray owned would come and go with a status item the
//! application may never show.
//!
//! JavaScript runs while winit already has the application borrowed, so the
//! menu cannot be replaced from the native function that receives the call.
//! Requests are queued here and applied immediately after that pump turn;
//! completions and native menu events travel back through the same queue and
//! are delivered at the top of a later frame.

use serde::Serialize;

use crate::MenuDefinition;

use super::command_channel::{CommandChannel, CommandRequest};

pub(crate) enum RequestKind {
    Configure(Vec<MenuDefinition>),
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

pub(crate) fn configure(entries: Vec<MenuDefinition>) -> u64 {
    request(RequestKind::Configure(entries))
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
            .map(|message| serde_json::to_value(message).expect("menu messages serialize"))
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
    fn requests_and_messages_keep_their_public_fifo_shape() {
        reset();
        // A replacement and a removal are two commands with two ids, so the
        // JavaScript promise each one settles is the one it queued.
        assert_eq!(configure(Vec::new()), 1);
        assert_eq!(remove(), 2);
        let described = take_requests()
            .iter()
            .map(|request| match &request.kind {
                RequestKind::Configure(entries) => {
                    (request.command_id, format!("configure {}", entries.len()))
                }
                RequestKind::Remove => (request.command_id, "remove".to_owned()),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            described,
            [(1, "configure 0".to_owned()), (2, "remove".to_owned())]
        );
        assert!(take_requests().is_empty());

        assert!(!pending());
        complete(1, Ok(()));
        complete(2, Err("no".into()));
        action("open".into(), None);
        action("dark".into(), Some(true));
        assert!(pending());
        assert_eq!(
            take_messages(),
            vec![
                serde_json::json!({ "type": "completion", "commandId": 1, "error": null }),
                serde_json::json!({ "type": "completion", "commandId": 2, "error": "no" }),
                serde_json::json!({ "type": "action", "id": "open" }),
                serde_json::json!({ "type": "action", "id": "dark", "checked": true }),
            ]
        );
        assert!(!pending());
    }
}

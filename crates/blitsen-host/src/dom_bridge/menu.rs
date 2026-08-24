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

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use serde::Serialize;

use crate::MenuDefinition;

pub(crate) enum RequestKind {
    Configure(Vec<MenuDefinition>),
    Remove,
}

pub(crate) struct Request {
    pub(crate) id: u64,
    pub(crate) kind: RequestKind,
}

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
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
    static REQUESTS: RefCell<VecDeque<Request>> = const { RefCell::new(VecDeque::new()) };
    static MESSAGES: RefCell<VecDeque<Message>> = const { RefCell::new(VecDeque::new()) };
}

fn request(kind: RequestKind) -> u64 {
    let id = NEXT_ID.get();
    NEXT_ID.set(id.saturating_add(1));
    REQUESTS.with_borrow_mut(|requests| requests.push_back(Request { id, kind }));
    id
}

pub(crate) fn configure(entries: Vec<MenuDefinition>) -> u64 {
    request(RequestKind::Configure(entries))
}

pub(crate) fn remove() -> u64 {
    request(RequestKind::Remove)
}

pub(crate) fn take_requests() -> Vec<Request> {
    REQUESTS.with_borrow_mut(|requests| requests.drain(..).collect())
}

pub(crate) fn complete(id: u64, result: Result<(), String>) {
    MESSAGES.with_borrow_mut(|messages| {
        messages.push_back(Message {
            kind: MessageKind::Completion {
                id,
                error: result.err(),
            },
        });
    });
}

pub(crate) fn action(id: String, checked: Option<bool>) {
    MESSAGES.with_borrow_mut(|messages| {
        messages.push_back(Message {
            kind: MessageKind::Action { id, checked },
        });
    });
}

pub(crate) fn pending() -> bool {
    MESSAGES.with_borrow(|messages| !messages.is_empty())
}

pub(crate) fn take_messages() -> Vec<serde_json::Value> {
    MESSAGES.with_borrow_mut(|messages| {
        messages
            .drain(..)
            .map(|message| serde_json::to_value(message).expect("menu messages serialize"))
            .collect()
    })
}

pub(crate) fn reset() {
    NEXT_ID.set(1);
    REQUESTS.with_borrow_mut(VecDeque::clear);
    MESSAGES.with_borrow_mut(VecDeque::clear);
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
                    (request.id, format!("configure {}", entries.len()))
                }
                RequestKind::Remove => (request.id, "remove".to_owned()),
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

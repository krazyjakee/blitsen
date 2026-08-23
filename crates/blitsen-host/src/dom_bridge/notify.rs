//! Main-thread handoff for `blitsen/notify` commands and lifecycle events.
//!
//! Platform callbacks never enter JavaScript. They wake winit and are drained by
//! the window session; this queue is the final hop into the next frame turn.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationAction {
    pub(crate) id: String,
    pub(crate) title: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationOptions {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) app_name: Option<String>,
    pub(crate) timeout: Option<u32>,
    pub(crate) urgency: String,
    pub(crate) icon: Option<String>,
    pub(crate) actions: Vec<NotificationAction>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) struct NotificationPatch {
    pub(crate) title: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) app_name: Option<String>,
    pub(crate) timeout: Option<u32>,
    pub(crate) urgency: Option<String>,
    pub(crate) icon: Option<String>,
    pub(crate) actions: Option<Vec<NotificationAction>>,
}

impl NotificationOptions {
    #[cfg(not(target_os = "windows"))]
    pub(crate) fn apply(&mut self, patch: NotificationPatch) {
        if let Some(value) = patch.title {
            self.title = value;
        }
        if let Some(value) = patch.body {
            self.body = value;
        }
        if let Some(value) = patch.app_name {
            self.app_name = Some(value);
        }
        if let Some(value) = patch.timeout {
            self.timeout = Some(value);
        }
        if let Some(value) = patch.urgency {
            self.urgency = value;
        }
        if let Some(value) = patch.icon {
            self.icon = Some(value);
        }
        if let Some(value) = patch.actions {
            self.actions = value;
        }
    }
}

pub(crate) enum RequestKind {
    RequestPermission,
    Show {
        public_id: String,
        options: NotificationOptions,
    },
    Update {
        public_id: String,
        patch: NotificationPatch,
    },
    Close {
        public_id: String,
    },
}

pub(crate) struct Request {
    pub(crate) command_id: u64,
    pub(crate) kind: RequestKind,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum MessageKind {
    Completion {
        #[serde(rename = "commandId")]
        id: u64,
        value: Value,
        error: Option<String>,
    },
    Show {
        id: String,
    },
    Click {
        id: String,
    },
    Action {
        id: String,
        action: String,
    },
    Close {
        id: String,
        reason: &'static str,
    },
    Error {
        id: String,
        message: String,
    },
}

#[derive(Serialize)]
struct Message {
    #[serde(flatten)]
    kind: MessageKind,
}

thread_local! {
    static NEXT_COMMAND_ID: Cell<u64> = const { Cell::new(1) };
    static NEXT_NOTIFICATION_ID: Cell<u64> = const { Cell::new(1) };
    static REQUESTS: RefCell<VecDeque<Request>> = const { RefCell::new(VecDeque::new()) };
    static MESSAGES: RefCell<VecDeque<Message>> = const { RefCell::new(VecDeque::new()) };
}

fn request(kind: RequestKind) -> u64 {
    let command_id = NEXT_COMMAND_ID.get();
    NEXT_COMMAND_ID.set(command_id.saturating_add(1));
    REQUESTS.with_borrow_mut(|requests| requests.push_back(Request { command_id, kind }));
    command_id
}

pub(crate) fn request_permission() -> u64 {
    request(RequestKind::RequestPermission)
}

pub(crate) fn show(options: NotificationOptions) -> u64 {
    let id = NEXT_NOTIFICATION_ID.get();
    NEXT_NOTIFICATION_ID.set(id.saturating_add(1));
    request(RequestKind::Show {
        public_id: format!("n{id}"),
        options,
    })
}

pub(crate) fn update(public_id: String, patch: NotificationPatch) -> u64 {
    request(RequestKind::Update { public_id, patch })
}

pub(crate) fn close(public_id: String) -> u64 {
    request(RequestKind::Close { public_id })
}

pub(crate) fn take_requests() -> Vec<Request> {
    REQUESTS.with_borrow_mut(|requests| requests.drain(..).collect())
}

fn push(kind: MessageKind) {
    MESSAGES.with_borrow_mut(|messages| messages.push_back(Message { kind }));
}

pub(crate) fn complete(command_id: u64, result: Result<Value, String>) {
    let (value, error) = match result {
        Ok(value) => (value, None),
        Err(error) => (Value::Null, Some(error)),
    };
    push(MessageKind::Completion {
        id: command_id,
        value,
        error,
    });
}

pub(crate) fn shown(id: String) {
    push(MessageKind::Show { id });
}

pub(crate) fn clicked(id: String) {
    push(MessageKind::Click { id });
}

pub(crate) fn action(id: String, action: String) {
    push(MessageKind::Action { id, action });
}

pub(crate) fn closed(id: String, reason: &'static str) {
    push(MessageKind::Close { id, reason });
}

pub(crate) fn failed(id: String, message: String) {
    push(MessageKind::Error { id, message });
}

pub(crate) fn pending() -> bool {
    MESSAGES.with_borrow(|messages| !messages.is_empty())
}

pub(crate) fn take_messages() -> Vec<Value> {
    MESSAGES.with_borrow_mut(|messages| {
        messages
            .drain(..)
            .map(|message| serde_json::to_value(message).expect("notification messages serialize"))
            .collect()
    })
}

pub(crate) fn reset() {
    NEXT_COMMAND_ID.set(1);
    NEXT_NOTIFICATION_ID.set(1);
    REQUESTS.with_borrow_mut(VecDeque::clear);
    MESSAGES.with_borrow_mut(VecDeque::clear);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_and_events_keep_fifo_shape() {
        reset();
        complete(3, Ok(serde_json::json!("n1")));
        shown("n1".into());
        clicked("n1".into());
        action("n1".into(), "reply".into());
        closed("n1".into(), "dismissed");
        failed("n2".into(), "service unavailable".into());
        assert_eq!(
            take_messages(),
            vec![
                serde_json::json!({"type":"completion","commandId":3,"value":"n1","error":null}),
                serde_json::json!({"type":"show","id":"n1"}),
                serde_json::json!({"type":"click","id":"n1"}),
                serde_json::json!({"type":"action","id":"n1","action":"reply"}),
                serde_json::json!({"type":"close","id":"n1","reason":"dismissed"}),
                serde_json::json!({"type":"error","id":"n2","message":"service unavailable"}),
            ]
        );
    }
}

//! Main-thread handoff for `blitsen/notify` commands and lifecycle events.
//!
//! Platform callbacks never enter JavaScript. They wake winit and are drained by
//! the window session; this queue is the final hop into the next frame turn.

use std::cell::Cell;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::command_channel::{CommandChannel, CommandRequest};

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

/// One notification activation, as the platform entry point recorded it (#252).
///
/// The wire type of a launch context, in the same sense `NotificationOptions` is
/// the wire type of a `show`: it crosses a process boundary as JSON, written by
/// whatever the platform started — a command line, an Android `Intent` extra —
/// and read here. `nonce` and `identity` never reach JavaScript; they belong to
/// the guard in `native_window::notify::activation` that decides whether this
/// envelope is a click nobody has been told about yet.
///
/// `id` is the session ID the notification was shown under, and after a cold
/// start it names a session that no longer exists — this process's own `n1`,
/// `n2`, … counter starts again at 1. That is the honest thing to carry:
/// correlating it with application state is the application's job, and minting a
/// fresh ID here would name a notification nobody ever saw.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Activation {
    /// Names this activation for as long as the guard remembers it.
    pub(crate) nonce: String,
    /// The installed application identity in force when it was recorded.
    pub(crate) identity: String,
    /// The notification's session ID, as the session that showed it named it.
    pub(crate) id: String,
    /// The native session that showed it, where the recorder can name one.
    ///
    /// Kept internal: it prevents an old session's `n1` dismissal from closing
    /// this session's unrelated `n1`, but is not application correlation data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session: Option<String>,
    /// The named action, absent for a body click.
    #[serde(default)]
    pub(crate) action: Option<String>,
    /// How the notification was dismissed, where the platform reports it.
    #[serde(default)]
    pub(crate) dismissed: Option<String>,
    /// Which platform's entry point produced this.
    pub(crate) platform: String,
    /// The entry point it came through, in that platform's own vocabulary: a
    /// desktop-entry name, an AppUserModelID, a bundle identifier, an
    /// application ID.
    pub(crate) entry: String,
}

impl Activation {
    /// Parses an envelope a platform entry point handed this process.
    ///
    /// Refused rather than repaired: an envelope is machine-written, and one
    /// that does not parse is a registration this build does not understand
    /// rather than a click to guess at.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text)
            .map_err(|error| format!("malformed notification activation: {error}"))
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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

pub(crate) type Request = CommandRequest<RequestKind>;

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
    /// The click that started this process, rather than one it observed.
    ///
    /// A kind of its own rather than a `click` with a flag, because the two are
    /// not the same event to a reader: the notification this names was shown by
    /// a session that has ended, so nothing in this document ever held a handle
    /// to it and no `Notification` object can be resolved from it. `platform`
    /// and `entry` are what the envelope carried about the entry point that
    /// produced it, which is the only way an application can tell a
    /// desktop-entry launch from a toast activation.
    Activation {
        id: String,
        action: Option<String>,
        reason: Option<String>,
        platform: String,
        entry: String,
    },
}

#[derive(Serialize)]
struct Message {
    #[serde(flatten)]
    kind: MessageKind,
}

thread_local! {
    static NEXT_NOTIFICATION_ID: Cell<u64> = const { Cell::new(1) };
    static CHANNEL: CommandChannel<RequestKind, Message> = const { CommandChannel::new() };
}

fn request(kind: RequestKind) -> u64 {
    CHANNEL.with(|channel| channel.request(kind))
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
    CHANNEL.with(CommandChannel::take_requests)
}

fn push(kind: MessageKind) {
    CHANNEL.with(|channel| channel.push(Message { kind }));
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

/// Queues the launch context this process was started with (#252).
///
/// Called before the document's scripts run, so the envelope is already in the
/// queue when a listener registered at the top level of a module subscribes, and
/// is drained by the same frame turn that drains every other notification event.
/// That ordering is the whole of the delivery contract: once, on a frame turn,
/// after a listener could have been added.
pub(crate) fn activated(activation: Activation) {
    push(MessageKind::Activation {
        id: activation.id,
        action: activation.action,
        // `reason` rather than `dismissed`, because it is the field a `close`
        // event already carries and the two answer the same question.
        reason: activation.dismissed,
        platform: activation.platform,
        entry: activation.entry,
    });
}

pub(crate) fn pending() -> bool {
    CHANNEL.with(CommandChannel::pending)
}

pub(crate) fn take_messages() -> Vec<Value> {
    CHANNEL.with(|channel| {
        channel
            .take_messages()
            .into_iter()
            .map(|message| serde_json::to_value(message).expect("notification messages serialize"))
            .collect()
    })
}

pub(crate) fn reset() {
    NEXT_NOTIFICATION_ID.set(1);
    CHANNEL.with(CommandChannel::reset);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The delivery contract of a cold-start activation (#252), at the queue
    /// that owns it.
    ///
    /// The activation is queued while the session is being opened and drained by
    /// the first frame turn after the document's scripts ran, so what has to
    /// hold here is that one push produces one message and that draining it
    /// leaves nothing for a later frame — a second turn must not repeat the
    /// click that started the process.
    #[test]
    fn an_activation_reaches_the_frame_turn_once() {
        reset();
        activated(Activation {
            nonce: "a1".into(),
            identity: "com.example.app".into(),
            id: "n7".into(),
            session: None,
            action: Some("reply".into()),
            dismissed: None,
            platform: "linux".into(),
            entry: "example".into(),
        });
        assert!(pending());
        assert_eq!(
            take_messages(),
            vec![serde_json::json!({
                "type": "activation", "id": "n7", "action": "reply", "reason": null,
                "platform": "linux", "entry": "example",
            })]
        );
        assert!(!pending());
        assert!(
            take_messages().is_empty(),
            "the next frame turn repeats nothing"
        );
    }

    /// A dismissal carries its reason where the platform reported one, and a
    /// body click names no action — the two identities #252 asks be preserved.
    #[test]
    fn a_body_click_and_a_dismissal_keep_their_identities() {
        reset();
        activated(Activation {
            nonce: "a2".into(),
            identity: "com.example.app".into(),
            id: "n1".into(),
            session: None,
            action: None,
            dismissed: Some("dismissed".into()),
            platform: "android".into(),
            entry: "com.example.app".into(),
        });
        assert_eq!(
            take_messages(),
            vec![serde_json::json!({
                "type": "activation", "id": "n1", "action": null, "reason": "dismissed",
                "platform": "android", "entry": "com.example.app",
            })]
        );
    }

    /// The same contract as the test above, through the runtime a document
    /// actually sees rather than through the queue behind it (#252).
    ///
    /// What the queue cannot answer on its own is the ordering that matters
    /// most: the activation is enqueued while the session is opening, *before*
    /// the document's scripts run, and a listener added at the top level of one
    /// of those scripts still has to receive it. So this boots a document the
    /// way `WindowSession::open` does — queue first, scripts second, frames
    /// third — and reads back what the listener saw on each frame.
    #[test]
    fn a_queued_activation_reaches_a_listener_on_a_frame_turn_and_no_later() {
        const SCRIPT: &str = r#"
            const { notify } = globalThis[Symbol.for("blitsen.native")];
            const seen = [];
            notify.onEvent(event => { if (event.type === "activation") seen.push(event); });
            const record = () => document.documentElement.setAttribute("data-activations",
                seen.map(event =>
                    `${event.type} ${event.id} ${event.action} ${event.reason} `
                    + `${event.platform} ${event.entry}`).join(", "));
            record();
            requestAnimationFrame(function tick() { record(); requestAnimationFrame(tick); });
        "#;

        reset();
        activated(Activation {
            nonce: "a1".into(),
            identity: "com.example.app".into(),
            id: "n3".into(),
            session: None,
            action: Some("open".into()),
            dismissed: None,
            platform: "linux".into(),
            entry: "example".into(),
        });
        // The same two installations a real session performs, in the same
        // order: the services own the timers and the console the bootstrap
        // captures as it loads, and the bridge is installed over them.
        let mut engine = blitsen_quickjs::QuickJs::new().expect("an engine");
        let _services =
            crate::runtime_services::RuntimeServices::install(&mut engine).expect("the services");
        let snapshots = crate::harness::execute_animation_harness(
            engine,
            "<!doctype html><html><body></body></html>".to_owned(),
            SCRIPT.to_owned(),
            2,
            200,
            100,
        )
        .expect("the harness document runs");

        // Read out of the serialized snapshot, which is the shape the harness
        // hands its JavaScript callers: nothing here needs a view of the tree
        // that a test suite does not already have.
        let recorded = |snapshot: &crate::harness::HarnessSnapshot| {
            serde_json::to_value(snapshot).expect("a snapshot serializes")["nodes"]
                .as_array()
                .expect("a snapshot lists its nodes")
                .iter()
                .find(|node| node["tag"] == "html")
                .and_then(|node| node["attributes"]["data-activations"].as_str())
                .expect("the document records what the listener saw")
                .to_owned()
        };
        assert_eq!(
            recorded(&snapshots[0]),
            "activation n3 open null linux example",
            "the launch context reaches a listener registered before the first frame"
        );
        assert_eq!(
            recorded(&snapshots[1]),
            recorded(&snapshots[0]),
            "a second frame turn must not repeat the click that started the process"
        );
    }

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

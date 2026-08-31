//! Linux specifics of the desktop notification backend: the freedesktop and
//! portal handles a shown notification is addressed through, and the watcher
//! that turns freedesktop responses into queued signals.

use std::collections::VecDeque;
use std::sync::Arc;

use notify_rust::NotificationResponse;
use parking_lot::Mutex;
use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

use super::{Signal, SignalKind, queue};
use crate::dom_bridge::notify::NotificationOptions;

pub(super) struct Record {
    pub(super) options: NotificationOptions,
    pub(super) handle: LinuxHandle,
    pub(super) token: u64,
}

pub(super) enum LinuxHandle {
    /// Development runs have no installed identity and retain the original
    /// freedesktop notification backend and its live-process response stream.
    Freedesktop(Box<notify_rust::NotificationHandle>),
    /// Packaged identities submit through the portal, which can D-Bus-activate
    /// the application after this process and its connection have exited.
    Portal,
}

pub(super) fn permission(request: bool) -> Result<Value, String> {
    let _ = request;
    Ok(json!("granted"))
}

/// Watches a freedesktop notification's response stream, reporting as
/// `public_id` at `token`.
pub(super) fn watch(
    native_id: u32,
    public_id: String,
    token: u64,
    signals: Arc<Mutex<VecDeque<Signal>>>,
    proxy: EventLoopProxy,
) {
    std::thread::spawn(move || {
        #[allow(deprecated)]
        let result = notify_rust::handle_action(native_id, |response| {
            #[allow(deprecated)]
            let response = match response {
                notify_rust::ActionResponse::Custom("default") => NotificationResponse::Default,
                notify_rust::ActionResponse::Custom(action) => {
                    NotificationResponse::Action((*action).to_owned())
                }
                notify_rust::ActionResponse::Closed(reason) => {
                    NotificationResponse::Closed(*reason)
                }
            };
            queue(
                &signals,
                &proxy,
                public_id.clone(),
                token,
                SignalKind::Response(response),
            );
        });
        if let Err(error) = result {
            queue(
                &signals,
                &proxy,
                public_id,
                token,
                SignalKind::Error(format!("could not observe notification response: {error}")),
            );
        }
    });
}

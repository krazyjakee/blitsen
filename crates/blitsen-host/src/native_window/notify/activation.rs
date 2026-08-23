//! The launch context a notification activation leaves behind (#252).
//!
//! Everything in `desktop.rs` and `android.rs` observes a notification the
//! *running* process showed. This file is the other half: a user who clicks a
//! notification belonging to an application that has exited is asking for it to
//! start, and what starts is a process with no memory of the notification at
//! all. The click therefore has to survive as data, be handed to the new
//! process by the platform entry point the packaging step registered, and reach
//! JavaScript once — not once per launch, and not once per Activity.
//!
//! # Why a persisted queue rather than a parameter
//!
//! An activation arrives before there is anything to deliver it to. On the
//! desktop it is on the command line of a process that has not yet parsed its
//! configuration; on Android it is an `Intent` extra that the platform hands to
//! *every* creation of the Activity, including the recreations a rotation
//! causes. Neither is a value that can be consumed where it appears, so both are
//! written into one file and read out of it — which is also what makes "already
//! delivered" a fact that outlives the process that delivered it.
//!
//! # Why a nonce
//!
//! The replay guard and the Android duplicate guard are the same guard. A
//! `.desktop` file re-run from a shell history, a `--notification-activation`
//! argument still sitting in a supervisor's command line, and an `Intent` the
//! platform re-delivers to a recreated Activity are all the same activation
//! offered a second time, and the only thing that can tell them apart from a
//! genuine second click is an identifier the *recorder* minted. So every
//! envelope carries one, and [`ActivationStore`] remembers the last
//! [`CONSUMED_LIMIT`] it has already handed over.
//!
//! # Why the identity is in the envelope as well as in the path
//!
//! The store lives in the application's own data directory, so a different
//! application cannot reach it. An *earlier install* of the same application
//! can: the directory outlives an uninstall, and an envelope left there names a
//! notification no session alive today ever showed. The identity is recorded
//! with each envelope and checked on the way out, so a store written under one
//! identity is discarded rather than replayed when the identity changes.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dom_bridge::notify::Activation;

/// What a process with no installed application identity is told.
///
/// The same shape #251 gave an unregistered Windows identity and #253 gave an
/// absent macOS bundle: a missing prerequisite, named as one, rather than a
/// verdict nobody reached. A development run is an interpreter executing a
/// script — the platform registered no entry point for it, so nothing can
/// address a notification back to it after it exits, and there is no directory
/// that is honestly *this application's* to remember a delivery in.
pub(crate) const NO_ACTIVATION_IDENTITY: &str = concat!(
    "a notification activation names the application it should be delivered to, and this process ",
    "has no installed application identity to match it against, so it cannot tell an activation ",
    "of its own from one addressed to something else. An identity is what `blitsen build ",
    "--bundle-id <id>` records and what the packaging step registers with the platform; a ",
    "development run has none, and notifications it shows are delivered only while it is running.",
);

/// The file the queue is kept in, inside the application's data directory.
const STORE_FILE: &str = "notification-activation.json";

/// How many delivered nonces the guard remembers.
///
/// The list only has to outlive the ways a *single* envelope can be offered
/// again — a command line re-run, an `Intent` re-delivered across recreation —
/// and every one of those repeats the most recent activation rather than an old
/// one. Sixty-four is far more than that needs and still bounds a file that is
/// otherwise written on every launch.
const CONSUMED_LIMIT: usize = 64;

/// The queue as it is written to disk.
#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Queue {
    /// Nonces already handed to JavaScript, most recent last.
    #[serde(default)]
    consumed: VecDeque<String>,
    /// Envelopes recorded but not yet handed over.
    #[serde(default)]
    pending: Vec<Activation>,
}

/// The activation queue belonging to one installed application identity.
pub(crate) struct ActivationStore {
    path: PathBuf,
    identity: String,
}

impl ActivationStore {
    /// The store `identity` keeps inside `directory`.
    pub(crate) fn new(directory: &Path, identity: &str) -> Self {
        Self {
            path: directory.join(STORE_FILE),
            identity: identity.to_owned(),
        }
    }

    /// A queue that has never been read is not an error: nothing has activated
    /// this application yet, which is the ordinary case on every launch.
    fn read(&self) -> Queue {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn write(&self, queue: &Queue) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "could not create {} for notification activations: {error}",
                    parent.display()
                )
            })?;
        }
        let bytes = serde_json::to_vec(queue).expect("an activation queue serializes");
        std::fs::write(&self.path, bytes).map_err(|error| {
            format!(
                "could not record a notification activation in {}: {error}",
                self.path.display()
            )
        })
    }

    /// Adds `activation` to the queue unless it has already been offered.
    ///
    /// Idempotent by nonce, which is what lets the caller record whatever the
    /// platform is holding — a command-line envelope, the `Intent` a recreated
    /// Activity was handed again — without first working out whether it is new.
    pub(crate) fn record(&self, activation: Activation) -> Result<(), String> {
        let mut queue = self.read();
        if queue.consumed.contains(&activation.nonce)
            || queue
                .pending
                .iter()
                .any(|pending| pending.nonce == activation.nonce)
        {
            return Ok(());
        }
        queue.pending.push(activation);
        self.write(&queue)
    }

    /// Removes and returns every activation this identity has not yet been given.
    ///
    /// The nonces go into the consumed list in the same write that empties the
    /// queue, so a process that is killed between reading and delivering loses
    /// the activation rather than replaying it. At-most-once is the safer side
    /// of "exactly once" for a launch context: a click delivered twice acts
    /// twice, and this is the click that opened a document or sent a reply.
    pub(crate) fn take(&self) -> Vec<Activation> {
        let mut queue = self.read();
        let pending = std::mem::take(&mut queue.pending);
        let emptied = !pending.is_empty();
        let mut taken = Vec::new();
        for activation in pending {
            // An envelope this identity cannot claim is dropped here rather
            // than left behind: it names a notification shown by an install
            // that no longer exists, and the only thing keeping it could do is
            // grow the file.
            if activation.identity != self.identity || queue.consumed.contains(&activation.nonce) {
                continue;
            }
            queue.consumed.push_back(activation.nonce.clone());
            taken.push(activation);
        }
        while queue.consumed.len() > CONSUMED_LIMIT {
            queue.consumed.pop_front();
        }
        // The ordinary launch queued nothing and empties nothing, and must not
        // pay a write for having looked.
        if emptied {
            let _ = self.write(&queue);
        }
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(directory: &Path) -> ActivationStore {
        ActivationStore::new(directory, "com.example.app")
    }

    fn activation(nonce: &str, action: Option<&str>) -> Activation {
        Activation {
            nonce: nonce.to_owned(),
            identity: "com.example.app".to_owned(),
            id: "n1".to_owned(),
            action: action.map(str::to_owned),
            dismissed: None,
            platform: "linux".to_owned(),
            entry: "example".to_owned(),
        }
    }

    /// A directory of this process's own, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("blitsen-activation-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("a scratch directory");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn an_envelope_carries_the_identity_of_the_click_it_describes() {
        let body = Activation::parse(
            r#"{"nonce":"a1","identity":"com.example.app","id":"n7","platform":"windows",
                "entry":"com.example.app"}"#,
        )
        .expect("a body click parses");
        assert_eq!(body.id, "n7");
        assert_eq!(body.action, None, "a body click names no action");
        assert_eq!(body.dismissed, None);

        let action = Activation::parse(
            r#"{"nonce":"a2","identity":"com.example.app","id":"n7","action":"reply",
                "platform":"android","entry":"com.example.app"}"#,
        )
        .expect("a named action parses");
        assert_eq!(action.action.as_deref(), Some("reply"));

        let dismissed = Activation::parse(
            r#"{"nonce":"a3","identity":"com.example.app","id":"n7","dismissed":"dismissed",
                "platform":"android","entry":"com.example.app"}"#,
        )
        .expect("a dismissal parses");
        assert_eq!(dismissed.dismissed.as_deref(), Some("dismissed"));

        // Round-tripping matters because the recorder and the reader are two
        // different processes, and on Android two different runtimes.
        let text = serde_json::to_string(&action).expect("an envelope serializes");
        assert_eq!(Activation::parse(&text).expect("it parses back"), action);
        assert!(Activation::parse("not an envelope").is_err());
    }

    #[test]
    fn an_activation_is_handed_over_once_and_never_again() {
        let scratch = Scratch::new("once");
        let store = store(&scratch.0);
        store
            .record(activation("a1", Some("open")))
            .expect("the queue is writable");

        let taken = store.take();
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].action.as_deref(), Some("open"));
        // The reload a development run performs, and the next launch after it.
        assert!(store.take().is_empty());
        assert!(
            ActivationStore::new(&scratch.0, "com.example.app")
                .take()
                .is_empty()
        );
    }

    #[test]
    fn the_same_envelope_offered_again_is_not_a_second_click() {
        let scratch = Scratch::new("replay");
        let store = store(&scratch.0);
        store.record(activation("a1", None)).expect("recorded");
        assert_eq!(store.take().len(), 1);

        // A command line re-run, or the Intent Android hands a recreated
        // Activity: the same nonce, offered by a platform that has no way to
        // know it was already delivered.
        store.record(activation("a1", None)).expect("recorded");
        assert!(
            store.take().is_empty(),
            "a nonce already delivered must not be delivered again"
        );

        // A genuine second click still is one.
        store.record(activation("a2", None)).expect("recorded");
        assert_eq!(store.take().len(), 1);
    }

    #[test]
    fn an_envelope_from_another_install_is_discarded_rather_than_delivered() {
        let scratch = Scratch::new("identity");
        let previous = ActivationStore::new(&scratch.0, "com.example.previous");
        previous
            .record(Activation {
                identity: "com.example.previous".to_owned(),
                ..activation("a1", None)
            })
            .expect("recorded");

        assert!(
            store(&scratch.0).take().is_empty(),
            "an activation addressed to an earlier install must not be delivered"
        );
        // And it is gone rather than left for the install it names to find: the
        // notification it describes belongs to a session nothing can reach.
        assert!(previous.take().is_empty());
    }

    #[test]
    fn ordering_survives_the_queue() {
        let scratch = Scratch::new("order");
        let store = store(&scratch.0);
        for nonce in ["a1", "a2", "a3"] {
            store
                .record(activation(nonce, Some(nonce)))
                .expect("recorded");
        }
        assert_eq!(
            store
                .take()
                .iter()
                .map(|activation| activation.nonce.clone())
                .collect::<Vec<_>>(),
            ["a1", "a2", "a3"],
            "activations reach JavaScript in the order the platform recorded them"
        );
    }

    #[test]
    fn the_refusal_without_an_identity_names_the_prerequisite() {
        assert!(NO_ACTIVATION_IDENTITY.contains("blitsen build --bundle-id <id>"));
        assert!(
            !NO_ACTIVATION_IDENTITY.contains("denied"),
            "a process with no identity has had nothing refused by anybody"
        );
    }
}

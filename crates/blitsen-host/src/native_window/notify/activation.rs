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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static SESSION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn session_token() -> String {
    let minted = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let sequence = SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{minted:x}-{:x}-{sequence:x}", std::process::id())
}

pub(crate) fn addresses_session(
    activation: &Activation,
    session: &str,
    active_record: bool,
) -> bool {
    active_record && activation.session.as_deref() == Some(session)
}

fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        std::fs::rename(source, destination)
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };

        let source = source
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let destination = destination
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: both paths are terminated UTF-16 buffers that remain alive
        // for the call. Rust's current `rename` does promise replacement on
        // Windows; this direct call names that semantic and also requests
        // WRITE_THROUGH so a successful replay-guard update has reached disk.
        let moved = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if moved == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "queue path has no parent")
    })?;
    std::fs::create_dir_all(parent)?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".notification-activation-{}-{sequence}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temporary, path)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(any(target_os = "android", test))]
fn safe_nonce(nonce: &str) -> bool {
    !nonce.is_empty()
        && nonce.len() <= 96
        && nonce
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

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

    /// A queue that has never existed is not an error: nothing has activated
    /// this application yet, which is the ordinary case on every launch.
    /// Every other read failure is fail-closed. Treating damaged bytes as an
    /// empty queue would let `record` overwrite the replay guard and let `take`
    /// claim that there was nothing to deliver.
    fn read(&self) -> Result<Queue, String> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Queue::default());
            }
            Err(error) => {
                return Err(format!(
                    "could not read notification activation queue {}: {error}",
                    self.path.display()
                ));
            }
        };
        serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "could not parse notification activation queue {}: {error}",
                self.path.display()
            )
        })
    }

    fn write(&self, queue: &Queue) -> Result<(), String> {
        let bytes = serde_json::to_vec(queue).expect("an activation queue serializes");
        atomic_write(&self.path, &bytes).map_err(|error| {
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
        let mut queue = self.read()?;
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

    /// Records the keyed envelope files a platform callback left in `inbox`.
    ///
    /// A successfully recorded file is removed. An unreadable file remains for
    /// a later frame or launch; malformed data is removed after one diagnostic
    /// because retrying bytes that cannot parse can never recover them.
    #[cfg(any(target_os = "android", test))]
    pub(crate) fn record_inbox(&self, inbox: &Path) -> Vec<(String, String)> {
        let mut failures = Vec::new();
        let mut entries = match std::fs::read_dir(inbox) {
            Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return failures,
            Err(error) => {
                failures.push((
                    String::new(),
                    format!(
                        "could not read notification activation inbox {}: {error}",
                        inbox.display()
                    ),
                ));
                return failures;
            }
        };
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    failures.push((
                        String::new(),
                        format!(
                            "could not read notification activation {}: {error}",
                            path.display()
                        ),
                    ));
                    continue;
                }
            };
            let activation = match serde_json::from_slice::<Activation>(&bytes) {
                Ok(activation) => activation,
                Err(error) => {
                    failures.push((
                        String::new(),
                        format!(
                            "could not parse notification activation {}: {error}",
                            path.display()
                        ),
                    ));
                    let _ = std::fs::remove_file(path);
                    continue;
                }
            };
            let id = activation.id.clone();
            let filename_nonce = path.file_stem().and_then(std::ffi::OsStr::to_str);
            if filename_nonce.is_none_or(|nonce| !safe_nonce(nonce) || nonce != activation.nonce) {
                failures.push((
                    id,
                    format!(
                        "notification activation {} does not match its safe nonce {:?}",
                        path.display(),
                        activation.nonce
                    ),
                ));
                let _ = std::fs::remove_file(path);
                continue;
            }
            match self.record(activation) {
                Ok(()) => {
                    if let Err(error) = std::fs::remove_file(&path) {
                        failures.push((
                            id,
                            format!(
                                "could not consume notification activation {}: {error}",
                                path.display()
                            ),
                        ));
                    }
                }
                Err(error) => failures.push((id, error)),
            }
        }
        failures
    }

    /// Removes and returns every activation this identity has not yet been given.
    ///
    /// The nonces go into the consumed list in the same write that empties the
    /// queue, so a process that is killed between reading and delivering loses
    /// the activation rather than replaying it. At-most-once is the safer side
    /// of "exactly once" for a launch context: a click delivered twice acts
    /// twice, and this is the click that opened a document or sent a reply.
    fn take_with_writer(
        &self,
        write: impl FnOnce(&Queue) -> Result<(), String>,
    ) -> Result<Vec<Activation>, String> {
        let mut queue = self.read()?;
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
            write(&queue)?;
        }
        Ok(taken)
    }

    pub(crate) fn take(&self) -> Result<Vec<Activation>, String> {
        self.take_with_writer(|queue| self.write(queue))
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
            session: None,
            action: action.map(str::to_owned),
            dismissed: None,
            platform: "linux".to_owned(),
            entry: "example".to_owned(),
        }
    }

    /// A unique directory removed when the test ends, including after a panic.
    struct Scratch(tempfile::TempDir);

    impl Scratch {
        fn new(name: &str) -> Self {
            Self(
                tempfile::Builder::new()
                    .prefix(&format!("blitsen-activation-{name}-"))
                    .tempdir()
                    .expect("a scratch directory"),
            )
        }

        fn path(&self) -> &Path {
            self.0.path()
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
        let store = store(scratch.path());
        store
            .record(activation("a1", Some("open")))
            .expect("the queue is writable");

        let taken = store.take().expect("the replay guard is durable");
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].action.as_deref(), Some("open"));
        // The reload a development run performs, and the next launch after it.
        assert!(
            store
                .take()
                .expect("the replay guard is durable")
                .is_empty()
        );
        assert!(
            ActivationStore::new(scratch.path(), "com.example.app")
                .take()
                .expect("the replay guard is durable")
                .is_empty()
        );
    }

    #[test]
    fn the_same_envelope_offered_again_is_not_a_second_click() {
        let scratch = Scratch::new("replay");
        let store = store(scratch.path());
        store.record(activation("a1", None)).expect("recorded");
        assert_eq!(store.take().expect("the replay guard is durable").len(), 1);

        // A command line re-run, or the Intent Android hands a recreated
        // Activity: the same nonce, offered by a platform that has no way to
        // know it was already delivered.
        store.record(activation("a1", None)).expect("recorded");
        assert!(
            store
                .take()
                .expect("the replay guard is durable")
                .is_empty(),
            "a nonce already delivered must not be delivered again"
        );

        // A genuine second click still is one.
        store.record(activation("a2", None)).expect("recorded");
        assert_eq!(store.take().expect("the replay guard is durable").len(), 1);
    }

    #[test]
    fn an_envelope_from_another_install_is_discarded_rather_than_delivered() {
        let scratch = Scratch::new("identity");
        let previous = ActivationStore::new(scratch.path(), "com.example.previous");
        previous
            .record(Activation {
                identity: "com.example.previous".to_owned(),
                ..activation("a1", None)
            })
            .expect("recorded");

        assert!(
            store(scratch.path())
                .take()
                .expect("the replay guard is durable")
                .is_empty(),
            "an activation addressed to an earlier install must not be delivered"
        );
        // And it is gone rather than left for the install it names to find: the
        // notification it describes belongs to a session nothing can reach.
        assert!(
            previous
                .take()
                .expect("the replay guard is durable")
                .is_empty()
        );
    }

    #[test]
    fn ordering_survives_the_queue() {
        let scratch = Scratch::new("order");
        let store = store(scratch.path());
        for nonce in ["a1", "a2", "a3"] {
            store
                .record(activation(nonce, Some(nonce)))
                .expect("recorded");
        }
        assert_eq!(
            store
                .take()
                .expect("the replay guard is durable")
                .iter()
                .map(|activation| activation.nonce.clone())
                .collect::<Vec<_>>(),
            ["a1", "a2", "a3"],
            "activations reach JavaScript in the order the platform recorded them"
        );
    }

    #[test]
    fn a_platform_inbox_is_drained_in_order_and_malformed_data_is_quarantined() {
        let scratch = Scratch::new("inbox");
        let inbox = scratch.path().join("inbox");
        std::fs::create_dir_all(&inbox).expect("an inbox");
        let first = activation("a1", None);
        let second = Activation {
            dismissed: Some("dismissed".into()),
            ..activation("a2", None)
        };
        std::fs::write(
            inbox.join("a1.json"),
            serde_json::to_vec(&first).expect("the envelope serializes"),
        )
        .expect("the first callback is persisted");
        std::fs::write(inbox.join("broken.json"), b"not json")
            .expect("the malformed callback is persisted");
        std::fs::write(
            inbox.join("a2.json"),
            serde_json::to_vec(&second).expect("the envelope serializes"),
        )
        .expect("the second callback is persisted");
        std::fs::write(inbox.join("still-writing.tmp"), b"ignored")
            .expect("an incomplete callback is present");

        let store = store(scratch.path());
        let failures = store.record_inbox(&inbox);
        assert_eq!(failures.len(), 1);
        assert!(
            failures[0]
                .1
                .contains("could not parse notification activation")
        );
        let taken = store.take().expect("the replay guard is durable");
        assert_eq!(
            taken
                .iter()
                .map(|activation| activation.nonce.as_str())
                .collect::<Vec<_>>(),
            vec!["a1", "a2"]
        );
        assert_eq!(taken[1].dismissed.as_deref(), Some("dismissed"));
        assert!(!inbox.join("a1.json").exists());
        assert!(!inbox.join("a2.json").exists());
        assert!(!inbox.join("broken.json").exists());
        assert!(inbox.join("still-writing.tmp").exists());
        assert!(store.record_inbox(&inbox).is_empty());
        assert!(
            store
                .take()
                .expect("the replay guard is durable")
                .is_empty(),
            "a drained callback is never replayed"
        );
    }

    #[test]
    fn an_inbox_filename_must_be_the_safe_nonce_inside_its_envelope() {
        let scratch = Scratch::new("inbox-nonce");
        let inbox = scratch.path().join("inbox");
        std::fs::create_dir_all(&inbox).expect("an inbox");
        let bytes = serde_json::to_vec(&activation("a1", None)).expect("an envelope");
        std::fs::write(inbox.join("a2.json"), &bytes).expect("a mismatched callback");
        std::fs::write(inbox.join("unsafe!.json"), &bytes).expect("an unsafe callback");

        let store = store(scratch.path());
        let failures = store.record_inbox(&inbox);
        assert_eq!(failures.len(), 2);
        assert!(
            failures
                .iter()
                .all(|(_, error)| error.contains("safe nonce"))
        );
        assert!(store.take().expect("the empty guard is durable").is_empty());
        assert!(
            std::fs::read_dir(&inbox)
                .expect("the inbox remains")
                .next()
                .is_none()
        );
    }

    #[test]
    fn a_failed_replay_guard_write_delivers_nothing_and_can_be_retried() {
        let scratch = Scratch::new("take-failure");
        let store = store(scratch.path());
        store.record(activation("a1", None)).expect("recorded");

        let result = store.take_with_writer(|_| Err("disk full".to_owned()));
        assert_eq!(
            result.expect_err("delivery must wait for durability"),
            "disk full"
        );
        assert_eq!(
            store.take().expect("the retry is durable").len(),
            1,
            "the activation remained pending after the refused delivery"
        );
    }

    #[test]
    fn queue_updates_leave_one_synced_target_and_no_temporary_files() {
        let scratch = Scratch::new("atomic");
        let store = store(scratch.path());
        for index in 0..32 {
            let nonce = format!("a{index:x}");
            store
                .record(activation(&nonce, None))
                .expect("atomic record");
            assert_eq!(store.take().expect("atomic consume").len(), 1);
            let _: Queue = serde_json::from_slice(
                &std::fs::read(&store.path).expect("the queue target exists"),
            )
            .expect("the target is always complete JSON");
        }
        assert!(
            std::fs::read_dir(scratch.path())
                .expect("the store directory")
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
        );
    }

    fn assert_unreadable_store_is_closed(store: &ActivationStore, expected: &str) {
        let record = store
            .record(activation("a2", None))
            .expect_err("record must not replace unreadable replay state");
        assert!(record.contains(expected), "{record}");
        let take = store
            .take()
            .expect_err("take must not report unreadable replay state as empty");
        assert!(take.contains(expected), "{take}");
    }

    #[test]
    fn a_truncated_queue_blocks_record_and_delivery_without_being_overwritten() {
        let scratch = Scratch::new("truncated");
        let store = store(scratch.path());
        let truncated = br#"{"pending":["#;
        std::fs::write(&store.path, truncated).expect("a truncated durable queue");

        assert_unreadable_store_is_closed(&store, "could not parse");
        assert_eq!(
            std::fs::read(&store.path).expect("the damaged queue remains for diagnosis"),
            truncated,
        );
    }

    #[test]
    fn a_structurally_corrupt_queue_is_not_treated_as_a_fresh_install() {
        let scratch = Scratch::new("corrupt");
        let store = store(scratch.path());
        let corrupt = br#"{"consumed":{},"pending":[]}"#;
        std::fs::write(&store.path, corrupt).expect("a corrupt durable queue");

        assert_unreadable_store_is_closed(&store, "could not parse");
        assert_eq!(
            std::fs::read(&store.path).expect("the corrupt queue remains for diagnosis"),
            corrupt,
        );
    }

    #[test]
    fn an_unreadable_queue_path_is_not_treated_as_an_absent_queue() {
        let scratch = Scratch::new("unreadable");
        let store = store(scratch.path());
        // A directory is a portable read failure even when the tests run as a
        // privileged user, unlike permission bits which root may bypass.
        std::fs::create_dir(&store.path).expect("an unreadable queue path");

        assert_unreadable_store_is_closed(&store, "could not read");
        assert!(
            store.path.is_dir(),
            "record must not replace the unreadable path"
        );
    }

    #[test]
    fn a_new_controller_generation_does_not_adopt_the_old_generations_same_id() {
        let first = session_token();
        let second = session_token();
        assert_ne!(first, second);
        let old = Activation {
            session: Some(first),
            ..activation("a1", None)
        };
        let current = Activation {
            nonce: "a2".into(),
            session: Some(second.clone()),
            ..old.clone()
        };
        assert!(!addresses_session(&old, &second, true));
        assert!(addresses_session(&current, &second, true));
        assert!(!addresses_session(&current, &second, false));
        assert_eq!(old.id, current.id, "both generations deliberately reuse n1");
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

//! Platform notification lifecycle.

mod activation;
#[cfg(target_os = "android")]
mod android;
#[cfg(not(target_os = "android"))]
mod desktop;

use std::sync::OnceLock;

pub(crate) use activation::{ActivationStore, NO_ACTIVATION_IDENTITY};
#[cfg(target_os = "android")]
pub(crate) use android::*;
#[cfg(not(target_os = "android"))]
pub(crate) use desktop::*;

use crate::dom_bridge::notify::Activation;
use crate::{ActivationEntryPoint, ActivationOptions};

/// The identity this process's notifications are filed and addressed under.
///
/// A process-wide `OnceLock` rather than session state, because that is what it
/// describes: the identity is what the packaging step registered *for this
/// executable*, every backend that names it does so from a free function with no
/// session to reach through, and a reload replaces the document rather than the
/// application. A second session in the same process keeps the first identity,
/// which is the only answer that could be true of both.
static ENTRY_POINT: OnceLock<ActivationEntryPoint> = OnceLock::new();

/// The registered entry point, or `None` for a run the platform knows nothing
/// about — a development run, or an export built without an identity.
pub(crate) fn entry_point() -> Option<&'static ActivationEntryPoint> {
    ENTRY_POINT.get()
}

/// Adopts the identity an export recorded, and registers it where a platform
/// wants registering.
///
/// Called once per session, before the first notification can be shown. The
/// registration is deliberately here and not in the packaging step: `blitsen
/// build` cross-compiles, so the machine that writes an artifact is often not
/// the machine that will run it, and what Windows needs written is a key in the
/// running user's own hive.
///
/// `display_name` is what the platform shows a user beside the notification —
/// the window title, which is the application's name as it wrote it. Windows is
/// the only platform that stores one against the identity; everywhere else the
/// name reaches the notification service with the notification itself.
fn adopt_entry_point(entry_point: &ActivationEntryPoint, display_name: &str) {
    let _ = ENTRY_POINT.set(entry_point.clone());
    #[cfg(target_os = "windows")]
    desktop::register_entry_point(display_name);
    #[cfg(not(target_os = "windows"))]
    let _ = display_name;
}

/// The identity the platform itself already knows this process by.
///
/// Only Android has one. Its application ID is what the package was installed
/// under and what the manifest declared, so asking the Activity for it cannot
/// disagree with the artifact — which is why the Android export records nothing
/// and a desktop export must. The desktop platforms have no equivalent: a
/// process there knows its executable path, and a path is not an identity.
#[cfg(not(target_os = "android"))]
fn installed_entry_point() -> Option<ActivationEntryPoint> {
    None
}

/// The directory this application's activation queue lives in.
///
/// Its own data directory, so an activation addressed to one application cannot
/// be read by another sharing the machine. Android resolves it through the
/// Activity instead, because the XDG variables the desktop answer reads are
/// unset there and would name a path nothing can write to.
#[cfg(not(target_os = "android"))]
fn store_directory(identity: &str) -> Result<std::path::PathBuf, String> {
    blitsen_platform::app::directory(blitsen_platform::app::Directory::Data, identity)
        .map_err(|error| error.message().to_owned())
}

/// The Activity's `filesDir`, which is already this application's alone — an
/// Android package cannot read another's, so the identity adds no separation
/// here and is checked only inside the envelopes.
#[cfg(target_os = "android")]
fn store_directory(_identity: &str) -> Result<std::path::PathBuf, String> {
    android::files_directory()
}

/// Records what the platform started this process with, and queues everything
/// this identity is owed for the first frame turn (#252).
///
/// Two kinds of failure, reported two ways. An envelope that does not parse, or
/// one handed to a process with no identity and nowhere to remember a delivery,
/// is a *launch* nobody can honour: `WindowSession::open` refuses it the way it
/// refuses any other unusable option, rather than starting an application that
/// silently ignores what the user clicked. A queue that cannot be written is not
/// that — the click is real and the application should still start — so it goes
/// to the notification error channel, naming the notification it could not
/// remember. An ordinary launch has read nothing and lost nothing, and neither
/// failure applies to it.
pub(crate) fn install(options: &ActivationOptions, display_name: &str) -> Result<(), String> {
    // What the export recorded, or — on the one platform that already holds the
    // answer — what the system installed this package as.
    let Some(entry_point) = options.entry_point.clone().or_else(installed_entry_point) else {
        return match options.launched_by {
            Some(_) => Err(NO_ACTIVATION_IDENTITY.to_owned()),
            None => Ok(()),
        };
    };
    adopt_entry_point(&entry_point, display_name);
    let directory = match store_directory(&entry_point.identity) {
        Ok(directory) => directory,
        Err(error) => {
            return match options.launched_by {
                Some(_) => Err(error),
                None => Ok(()),
            };
        }
    };
    let store = ActivationStore::new(&directory, &entry_point.identity);
    if let Some(text) = &options.launched_by {
        let activation = Activation::parse(text)?;
        let id = activation.id.clone();
        if let Err(error) = store.record(activation) {
            crate::dom_bridge::notify::failed(id, error);
        }
    }
    #[cfg(target_os = "android")]
    record_intent_activation(&store);
    for activation in store.take() {
        crate::dom_bridge::notify::activated(activation);
    }
    Ok(())
}

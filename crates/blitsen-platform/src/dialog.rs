//! Native file and message dialogs, backed by `rfd`.
//!
//! A dialog future is driven on a thread of its own and its outcome is queued
//! here for the runtime to collect, rather than blocking the caller. The reason
//! is the frame loop: the thread a `native:dialog` call arrives on is the thread
//! that pumps winit, so blocking it would stop the application painting for as long
//! as the dialog is on screen — and a client that stops reading its display
//! socket is one X11 and Wayland compositors are entitled to grey out.
//!
//! The asynchronous backend presents macOS panels on the main thread without
//! blocking its event loop, uses a COM worker on Windows, and speaks the XDG
//! portal on Linux. The parent window handle makes each dialog modal.
//!
//! Android is off them too, and not only because there is no portal to speak to.
//! Its file chooser is an `Intent` handed to the system and answered by a
//! different activity, so the answer arrives through the lifecycle rather than
//! through a queue this process drains. The shape above — start the dialog
//! elsewhere, let the frame loop keep turning, collect the outcome on a later
//! turn — is the right shape for that, which is worth recording rather than
//! rediscovering; what is missing is an entry point holding an `AndroidApp` to
//! route the result back through (#142). Absent until there is one (#147).
//!
//! The predicate that keeps this module off a platform is spelled in more than
//! one place and they have to move together. #139 is what happens when they do
//! not: `all(unix, not(target_os = "macos"))` claimed to name the portal
//! platforms and matched Android as well.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use parking_lot::Mutex;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::PlatformError;

/// Which file dialog to show.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKind {
    /// One existing file.
    OpenFile,
    /// Any number of existing files.
    OpenFiles,
    /// A path to write to, which need not exist yet.
    SaveFile,
    /// One existing directory.
    OpenFolder,
    /// Any number of existing directories.
    OpenFolders,
}

/// One named group of extensions the file dialog offers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Filter {
    /// What the group is called in the dialog's filter list.
    pub name: String,
    /// Extensions without their dot.
    pub extensions: Vec<String>,
}

/// A file dialog to show.
#[derive(Clone, Debug, Default)]
pub struct FileRequest {
    /// Dialog title, or the platform's own wording.
    pub title: Option<String>,
    /// Directory to open in.
    pub directory: Option<PathBuf>,
    /// File name to suggest, for a save dialog.
    pub file_name: Option<String>,
    /// Extension groups to offer, in order.
    pub filters: Vec<Filter>,
}

/// How urgent a message dialog is, which the platform draws as an icon.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Level {
    /// Ordinary information.
    #[default]
    Info,
    /// Something the user should look at.
    Warning,
    /// Something that went wrong.
    Error,
}

/// The buttons a message dialog offers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Buttons {
    /// Acknowledgement only.
    #[default]
    Ok,
    /// Accept or dismiss.
    OkCancel,
    /// A yes/no question.
    YesNo,
    /// A yes/no question that can also be dismissed.
    YesNoCancel,
}

/// A message dialog to show.
#[derive(Clone, Debug, Default)]
pub struct MessageRequest {
    /// Dialog title.
    pub title: String,
    /// Body text.
    pub message: String,
    /// How urgent it is.
    pub level: Level,
    /// Which buttons to offer.
    pub buttons: Buttons,
}

/// The button a message dialog was dismissed with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Button {
    /// The affirmative button of an acknowledgement.
    Ok,
    /// Dismissed, including by closing the dialog.
    Cancel,
    /// The affirmative button of a question.
    Yes,
    /// The negative button of a question.
    No,
}

/// What a dialog was answered with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The paths chosen, empty when the dialog was dismissed.
    Paths(Vec<PathBuf>),
    /// The button pressed.
    Button(Button),
}

/// A dialog that has closed, addressed by the id [`open_file`] returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    /// The id of the request this answers.
    pub id: u64,
    /// What the user did.
    pub outcome: Outcome,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Dialogs opened whose outcome has not been queued yet.
static OPEN: AtomicUsize = AtomicUsize::new(0);
/// The deliberately narrow exception to the DOM bridge `CommandChannel`.
///
/// A dialog command is submitted immediately and its answer is produced on a
/// worker thread, so this queue must be `Sync` and process-visible. The shared
/// bridge channel is thread-local because JavaScript and the window session
/// hand work to each other on one thread. Only completed outcomes cross this
/// boundary; request IDs, pending worker accounting and the mutex therefore
/// stay here rather than pretending the two concurrency models are the same.
/// Completed work is independent: one dialog thread failing must not poison
/// the queue for every later dialog.
static COMPLETED: Mutex<Vec<Completion>> = Mutex::new(Vec::new());

/// Shows a file dialog of `kind`, returning the id its completion carries.
pub fn open_file<W>(
    kind: FileKind,
    request: &FileRequest,
    parent: Option<&W>,
) -> Result<u64, PlatformError>
where
    W: HasWindowHandle + HasDisplayHandle + ?Sized,
{
    session()?;
    let dialog = file_dialog(request, parent);
    show(move || Outcome::Paths(pick(kind, dialog)))
}

/// Shows a message dialog, returning the id its completion carries.
pub fn open_message<W>(request: &MessageRequest, parent: Option<&W>) -> Result<u64, PlatformError>
where
    W: HasWindowHandle + HasDisplayHandle + ?Sized,
{
    session()?;
    let dialog = message_dialog(request, parent);
    show(move || Outcome::Button(button(dialog.show())))
}

/// Drains the dialogs that have closed since the last call.
pub fn take() -> Vec<Completion> {
    std::mem::take(&mut *COMPLETED.lock())
}

/// Whether any dialog is open or any outcome is waiting to be read.
///
/// True for an open dialog as well as a finished one because nothing else wakes
/// the frame loop: the outcome arrives on a thread, and the turn that collects
/// it only happens while the loop believes it has work.
pub fn pending() -> bool {
    OPEN.load(Ordering::Acquire) > 0 || !COMPLETED.lock().is_empty()
}

/// Runs one dialog on a thread of its own and queues what it answered.
fn show(dialog: impl FnOnce() -> Outcome + Send + 'static) -> Result<u64, PlatformError> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    OPEN.fetch_add(1, Ordering::Release);
    std::thread::Builder::new()
        .name("blitsen-dialog".to_owned())
        .spawn(move || {
            let outcome = dialog();
            // Queued before the count drops, so a caller polling between the two
            // never sees an idle runtime with an unread answer in it.
            COMPLETED.lock().push(Completion { id, outcome });
            OPEN.fetch_sub(1, Ordering::Release);
        })
        .map_err(|error| {
            OPEN.fetch_sub(1, Ordering::Release);
            PlatformError::new(format!("could not show the dialog: {error}"))
        })?;
    Ok(id)
}

/// Refuses before opening anything when there is no session to open it in.
///
/// Without this the portal fails, `rfd` reports the same `None` a user pressing
/// Cancel produces, and the application is told its dialog was dismissed by
/// someone who never saw it.
#[cfg(target_os = "linux")]
fn session() -> Result<(), PlatformError> {
    let displayed = ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|variable| std::env::var_os(variable).is_some_and(|value| !value.is_empty()));
    if displayed {
        return Ok(());
    }
    Err(PlatformError::new(
        "there is no desktop session to show a dialog in: neither DISPLAY nor WAYLAND_DISPLAY \
         is set",
    ))
}

#[cfg(not(target_os = "linux"))]
fn session() -> Result<(), PlatformError> {
    Ok(())
}

fn file_dialog<W>(request: &FileRequest, parent: Option<&W>) -> rfd::AsyncFileDialog
where
    W: HasWindowHandle + HasDisplayHandle + ?Sized,
{
    let mut dialog = rfd::AsyncFileDialog::new();
    if let Some(title) = &request.title {
        dialog = dialog.set_title(title);
    }
    if let Some(directory) = &request.directory {
        dialog = dialog.set_directory(directory);
    }
    if let Some(file_name) = &request.file_name {
        dialog = dialog.set_file_name(file_name);
    }
    for filter in &request.filters {
        dialog = dialog.add_filter(&filter.name, &filter.extensions);
    }
    if let Some(parent) = parent {
        dialog = dialog.set_parent(parent);
    }
    dialog
}

fn pick(kind: FileKind, dialog: rfd::AsyncFileDialog) -> Vec<PathBuf> {
    pollster::block_on(async move {
        match kind {
            FileKind::OpenFile => one_path(dialog.pick_file().await),
            FileKind::OpenFiles => paths(dialog.pick_files().await),
            FileKind::SaveFile => one_path(dialog.save_file().await),
            FileKind::OpenFolder => one_path(dialog.pick_folder().await),
            FileKind::OpenFolders => paths(dialog.pick_folders().await),
        }
    })
}

fn one_path(file: Option<rfd::FileHandle>) -> Vec<PathBuf> {
    file.into_iter()
        .map(|file| file.path().to_owned())
        .collect()
}

fn paths(files: Option<Vec<rfd::FileHandle>>) -> Vec<PathBuf> {
    files
        .unwrap_or_default()
        .into_iter()
        .map(|file| file.path().to_owned())
        .collect()
}

fn message_dialog<W>(request: &MessageRequest, parent: Option<&W>) -> rfd::MessageDialog
where
    W: HasWindowHandle + HasDisplayHandle + ?Sized,
{
    let mut dialog = rfd::MessageDialog::new()
        .set_title(&request.title)
        .set_description(&request.message)
        .set_level(match request.level {
            Level::Info => rfd::MessageLevel::Info,
            Level::Warning => rfd::MessageLevel::Warning,
            Level::Error => rfd::MessageLevel::Error,
        })
        .set_buttons(match request.buttons {
            Buttons::Ok => rfd::MessageButtons::Ok,
            Buttons::OkCancel => rfd::MessageButtons::OkCancel,
            Buttons::YesNo => rfd::MessageButtons::YesNo,
            Buttons::YesNoCancel => rfd::MessageButtons::YesNoCancel,
        });
    if let Some(parent) = parent {
        dialog = dialog.set_parent(parent);
    }
    dialog
}

/// Custom answers cannot arrive: no request here offers a custom button.
fn button(result: rfd::MessageDialogResult) -> Button {
    match result {
        rfd::MessageDialogResult::Ok => Button::Ok,
        rfd::MessageDialogResult::Yes => Button::Yes,
        rfd::MessageDialogResult::No => Button::No,
        rfd::MessageDialogResult::Cancel | rfd::MessageDialogResult::Custom(_) => Button::Cancel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue has to answer honestly with nothing in it, because every frame
    /// turn asks it that before a dialog is ever opened.
    #[test]
    fn an_unused_queue_is_idle_and_empty() {
        assert!(!pending());
        assert!(take().is_empty());
    }

    /// A dialog nobody could have seen must not report that it was dismissed.
    #[test]
    fn a_dialog_needs_a_session_to_appear_in() {
        match session() {
            Ok(()) => assert!(
                std::env::var_os("DISPLAY").is_some()
                    || std::env::var_os("WAYLAND_DISPLAY").is_some()
            ),
            Err(error) => assert!(error.message().contains("no desktop session")),
        }
    }
}

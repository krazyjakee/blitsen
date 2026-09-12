//! Handing a URL or a path to the desktop: the browser, the registered
//! application, or the file manager with an item selected (#384).
//!
//! Three operations with three different consequences, kept apart on purpose.
//! Opening a URL sends the person to their browser; opening a path runs
//! whatever the desktop associates with the file, which for a script or an
//! executable is the file itself; revealing a path only shows it, and runs
//! nothing. None of them is document navigation — the window keeps its
//! document — and none of them is a shell: every argument crosses as one
//! `argv` element or one API parameter, never as text a shell would parse.
//!
//! A URL is refused unless its scheme is one the desktop hands to a browser or
//! a mail client. That refusal happens here, synchronously, before anything is
//! spawned: `javascript:`, `file:` and a custom scheme registered by some other
//! application are exactly the strings this surface exists not to pass on.
//!
//! The dispatch itself runs on a thread and its outcome is queued for the frame
//! loop to collect, the way `dialog` does it: `xdg-open` may block for as long
//! as the handler it found, and the thread these calls arrive on is the one
//! that paints. Absent on Android, where opening a URL or a file is an `Intent`
//! the Activity sends and there is no file manager to reveal anything in.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use parking_lot::Mutex;

use crate::PlatformError;

/// The URL schemes `open_external` hands to the desktop.
///
/// Web pages and mail: the two things every desktop routes to a browser or a
/// mail client the person chose. Everything else — including `file:`, which is
/// `open_path` with the safety of a URL parser removed — is refused.
pub const EXTERNAL_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// The longest URL this will hand on. Longer than any real link; shorter than
/// a command line the desktop tools would truncate or reject.
const MAX_URL_LENGTH: usize = 8 * 1024;

/// Why the desktop did not do what was asked, named for the `DOMException` the
/// bridge raises.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure {
    /// The path does not exist.
    NotFound(String),
    /// The desktop has no handler for this URL or file.
    NotSupported(String),
    /// The handler was found and failed, or could not be reached.
    Operation(String),
}

impl Failure {
    /// The `DOMException` name an application branches on.
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "NotFoundError",
            Self::NotSupported(_) => "NotSupportedError",
            Self::Operation(_) => "OperationError",
        }
    }

    /// The text written for a person reading a log.
    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(message) | Self::NotSupported(message) | Self::Operation(message) => {
                message
            }
        }
    }
}

/// An operation that has finished, addressed by the id its request returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    /// The id of the request this answers.
    pub id: u64,
    /// What happened.
    pub outcome: Result<(), Failure>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Operations started whose outcome has not been queued yet.
static OPEN: AtomicUsize = AtomicUsize::new(0);
/// Outcomes produced on worker threads, waiting for a frame turn. The same
/// narrow exception to the bridge's thread-local channel that `dialog` makes,
/// for the same reason: the answer is produced on another thread.
static COMPLETED: Mutex<Vec<Completion>> = Mutex::new(Vec::new());

/// Opens a URL in the application the desktop associates with its scheme.
///
/// Refuses a URL whose scheme is not in [`EXTERNAL_SCHEMES`] before anything
/// is spawned. Returns the id the completion will carry.
pub fn open_external(url: &str) -> Result<u64, PlatformError> {
    let url = external_url(url)?;
    dispatch(move || platform::open_url(&url))
}

/// Opens a file or directory in the application the desktop associates with it.
///
/// The path must be absolute. It is checked for existence on the worker, so a
/// missing path is a `NotFound` completion rather than a refusal here.
pub fn open_path(path: &str) -> Result<u64, PlatformError> {
    let path = local_path(path)?;
    dispatch(move || {
        exists(&path)?;
        platform::open_path(&path)
    })
}

/// Reveals a file or directory in the desktop's file manager, selected.
///
/// Nothing is executed: a script revealed is a script shown. The path must be
/// absolute and, on the worker, exist.
pub fn show_item_in_folder(path: &str) -> Result<u64, PlatformError> {
    let path = local_path(path)?;
    dispatch(move || {
        exists(&path)?;
        platform::show_item_in_folder(&path)
    })
}

/// Drains the operations that have finished since the last call.
pub fn take() -> Vec<Completion> {
    std::mem::take(&mut *COMPLETED.lock())
}

/// Whether any operation is running or any outcome is waiting to be read.
pub fn pending() -> bool {
    OPEN.load(Ordering::Acquire) > 0 || !COMPLETED.lock().is_empty()
}

/// Checks a URL against the scheme allow-list, returning it unchanged.
///
/// The check is textual and deliberately strict: a scheme per RFC 3986, no
/// whitespace or control characters anywhere, a host for the web schemes and
/// an address for `mailto`. What is not checked is the rest of the URL's
/// grammar — that is the browser's, and a browser handles a strange URL
/// better than this module could refuse one.
pub fn external_url(url: &str) -> Result<String, PlatformError> {
    if url.is_empty() {
        return Err(PlatformError::new("an external URL must not be empty"));
    }
    if url.len() > MAX_URL_LENGTH {
        return Err(PlatformError::new(format!(
            "an external URL must be at most {MAX_URL_LENGTH} bytes"
        )));
    }
    if url
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(PlatformError::new(
            "an external URL must not contain whitespace or control characters",
        ));
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return Err(PlatformError::new(format!("{url:?} has no URL scheme")));
    };
    let well_formed = scheme
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "+-.".contains(character));
    if !well_formed {
        return Err(PlatformError::new(format!("{url:?} has no URL scheme")));
    }
    let scheme = scheme.to_ascii_lowercase();
    if !EXTERNAL_SCHEMES.contains(&scheme.as_str()) {
        return Err(PlatformError::new(format!(
            "{scheme}: is not a scheme openExternal hands to the desktop; \
             http, https and mailto are"
        )));
    }
    let host = match scheme.as_str() {
        "mailto" => rest,
        _ => rest
            .strip_prefix("//")
            .map(|authority| authority.split(['/', '?', '#']).next().unwrap_or(""))
            .unwrap_or(""),
    };
    if host.is_empty() {
        return Err(PlatformError::new(format!(
            "{url:?} names no {}",
            if scheme == "mailto" {
                "address"
            } else {
                "host"
            }
        )));
    }
    Ok(url.to_owned())
}

/// Checks that a string is an absolute path with nothing a path cannot carry.
pub fn local_path(path: &str) -> Result<PathBuf, PlatformError> {
    if path.is_empty() {
        return Err(PlatformError::new("a path must not be empty"));
    }
    if path.contains('\0') {
        return Err(PlatformError::new("a path must not contain a NUL byte"));
    }
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(PlatformError::new(format!(
            "{} is not an absolute path; the desktop has no working directory to resolve it \
             against",
            path.display()
        )));
    }
    Ok(path)
}

fn exists(path: &Path) -> Result<(), Failure> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(Failure::NotFound(
            format!("{} does not exist", path.display()),
        )),
        Err(error) => Err(Failure::Operation(format!(
            "{} could not be read: {error}",
            path.display()
        ))),
    }
}

/// Runs one operation on a thread of its own and queues what it answered.
fn dispatch(
    operation: impl FnOnce() -> Result<(), Failure> + Send + 'static,
) -> Result<u64, PlatformError> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    OPEN.fetch_add(1, Ordering::Release);
    std::thread::Builder::new()
        .name("blitsen-shell".to_owned())
        .spawn(move || {
            let outcome = operation();
            // Queued before the count drops, so a caller polling between the two
            // never sees an idle runtime with an unread answer in it.
            COMPLETED.lock().push(Completion { id, outcome });
            OPEN.fetch_sub(1, Ordering::Release);
        })
        .map_err(|error| {
            OPEN.fetch_sub(1, Ordering::Release);
            PlatformError::new(format!("could not start the desktop hand-off: {error}"))
        })?;
    Ok(id)
}

/// Runs a desktop helper with its arguments as `argv`, reporting failure by
/// exit status and whatever it wrote to stderr.
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod helper {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::Failure;

    /// A finished helper: its exit code, and what it said.
    pub(super) struct Exit {
        pub(super) code: Option<i32>,
        pub(super) stderr: String,
    }

    /// Runs a helper and waits for it to finish.
    ///
    /// `settle` bounds the wait: a helper still running past it is one that
    /// found its handler and is now running *that*, which `xdg-open` does when
    /// no desktop's own opener is installed to hand off to. The call then
    /// reports success and the thread stays behind only to reap the process,
    /// so nothing is left as a zombie.
    pub(super) fn run(
        program: &str,
        arguments: &[&std::ffi::OsStr],
        settle: Option<Duration>,
    ) -> Result<Option<Exit>, Failure> {
        let mut child = Command::new(program)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => {
                    Failure::NotSupported(format!("{program} is not installed"))
                }
                _ => Failure::Operation(format!("{program} could not be started: {error}")),
            })?;
        let stderr = child.stderr.take();
        let read_stderr = move || {
            let mut text = String::new();
            if let Some(mut stderr) = stderr {
                let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
            }
            text.trim().to_owned()
        };
        let Some(settle) = settle else {
            let status = child
                .wait()
                .map_err(|error| Failure::Operation(format!("{program} was lost: {error}")))?;
            return Ok(Some(Exit {
                code: status.code(),
                stderr: read_stderr(),
            }));
        };
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Ok(Some(Exit {
                        code: status.code(),
                        stderr: read_stderr(),
                    }));
                }
                Ok(None) if started.elapsed() < settle => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Ok(None) => {
                    // Reap it when it does finish, on a thread nobody waits for;
                    // its stderr pipe closes with it.
                    std::thread::Builder::new()
                        .name("blitsen-shell-reaper".to_owned())
                        .spawn(move || {
                            let _ = child.wait();
                            drop(read_stderr());
                        })
                        .ok();
                    return Ok(None);
                }
                Err(error) => {
                    return Err(Failure::Operation(format!("{program} was lost: {error}")));
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::ffi::OsStr;
    use std::path::Path;
    use std::time::Duration;

    use super::{Failure, helper};

    /// How long `xdg-open` is given to report a missing handler before a
    /// helper still running is taken to be the handler itself.
    const SETTLE: Duration = Duration::from_millis(1500);

    /// `xdg-open`'s documented exit codes.
    fn xdg_open_outcome(exit: Option<helper::Exit>, what: &str) -> Result<(), Failure> {
        let Some(exit) = exit else {
            return Ok(());
        };
        let detail = |fallback: &str| {
            if exit.stderr.is_empty() {
                fallback.to_owned()
            } else {
                exit.stderr.clone()
            }
        };
        match exit.code {
            Some(0) => Ok(()),
            Some(2) => Err(Failure::NotFound(detail(&format!("{what} was not found")))),
            Some(3) => Err(Failure::NotSupported(detail(&format!(
                "no desktop handler is installed for {what}"
            )))),
            Some(code) => Err(Failure::Operation(detail(&format!(
                "xdg-open exited with status {code} for {what}"
            )))),
            None => Err(Failure::Operation(detail(&format!(
                "xdg-open was killed by a signal while opening {what}"
            )))),
        }
    }

    pub(super) fn open_url(url: &str) -> Result<(), Failure> {
        let exit = helper::run("xdg-open", &[OsStr::new(url)], Some(SETTLE))?;
        xdg_open_outcome(exit, url)
    }

    pub(super) fn open_path(path: &Path) -> Result<(), Failure> {
        let exit = helper::run("xdg-open", &[path.as_os_str()], Some(SETTLE))?;
        xdg_open_outcome(exit, &path.display().to_string())
    }

    /// The file manager's own selection interface, then the directory.
    ///
    /// `org.freedesktop.FileManager1` is what Nautilus, Dolphin, Thunar and
    /// Nemo implement for exactly this, and the session bus starts the
    /// registered one when it is not running. A desktop without one gets the
    /// containing directory opened, which shows the item without selecting it.
    pub(super) fn show_item_in_folder(path: &Path) -> Result<(), Failure> {
        if file_manager_show_items(path).is_ok() {
            return Ok(());
        }
        let directory = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let exit = helper::run("xdg-open", &[directory.as_os_str()], Some(SETTLE))?;
        xdg_open_outcome(exit, &directory.display().to_string())
    }

    fn file_manager_show_items(path: &Path) -> Result<(), String> {
        use zbus::blocking::{Connection, Proxy};

        let uri = super::file_uri(path);
        let connection = Connection::session().map_err(|error| error.to_string())?;
        let proxy = Proxy::new(
            &connection,
            "org.freedesktop.FileManager1",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1",
        )
        .map_err(|error| error.to_string())?;
        proxy
            .call_method("ShowItems", &(vec![uri], ""))
            .map(drop)
            .map_err(|error| error.to_string())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::OsStr;
    use std::path::Path;

    use super::{Failure, helper};

    /// `open(1)` hands off to Launch Services and returns at once, so its exit
    /// status is the answer rather than a wait on the application it started.
    fn open(arguments: &[&OsStr], what: &str) -> Result<(), Failure> {
        let exit = helper::run("open", arguments, None)?
            .expect("a helper waited for without a settle bound reports its exit");
        match exit.code {
            Some(0) => Ok(()),
            _ => {
                let message = if exit.stderr.is_empty() {
                    format!("the desktop could not open {what}")
                } else {
                    exit.stderr
                };
                // `open` says "Unable to find application" when nothing is
                // registered for the scheme or type, which is the one outcome
                // an application can do something about: offer another route.
                if message.contains("Unable to find application")
                    || message.contains("No application")
                {
                    Err(Failure::NotSupported(message))
                } else {
                    Err(Failure::Operation(message))
                }
            }
        }
    }

    pub(super) fn open_url(url: &str) -> Result<(), Failure> {
        open(&[OsStr::new(url)], url)
    }

    pub(super) fn open_path(path: &Path) -> Result<(), Failure> {
        open(&[path.as_os_str()], &path.display().to_string())
    }

    pub(super) fn show_item_in_folder(path: &Path) -> Result<(), Failure> {
        open(
            &[OsStr::new("-R"), path.as_os_str()],
            &path.display().to_string(),
        )
    }
}

#[cfg(windows)]
mod platform {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::Shell::{
        ILCreateFromPathW, ILFree, SE_ERR_ACCESSDENIED, SE_ERR_FNF, SE_ERR_NOASSOC, SE_ERR_PNF,
        SHOpenFolderAndSelectItems, ShellExecuteW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use super::Failure;

    fn wide(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        text.as_ref()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Runs `body` inside a COM apartment, which `ShellExecuteW` documents as
    /// its precondition and this worker thread does not otherwise have.
    fn in_apartment<T>(body: impl FnOnce() -> T) -> T {
        // SAFETY: `CoInitializeEx` and `CoUninitialize` are paired on this thread.
        let initialised = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        } >= 0;
        let result = body();
        if initialised {
            // SAFETY: balances the successful `CoInitializeEx` above.
            unsafe { CoUninitialize() };
        }
        result
    }

    /// `ShellExecuteW`'s "open" verb, which for a URL is the scheme's handler
    /// and for a path the file association.
    fn shell_execute(target: &std::ffi::OsStr, what: &str) -> Result<(), Failure> {
        let verb = wide("open");
        let file = wide(target);
        let instance = in_apartment(|| {
            // SAFETY: every pointer is to a NUL-terminated buffer that outlives
            // the call; the null ones are documented as optional.
            unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    verb.as_ptr(),
                    file.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    SW_SHOWNORMAL,
                )
            }
        });
        // Documented as an integer disguised as an instance handle: above 32 is
        // success, and the small values are the error codes below.
        let code = instance as usize as u32;
        if code > 32 {
            return Ok(());
        }
        Err(match code {
            SE_ERR_FNF | SE_ERR_PNF => Failure::NotFound(format!("{what} was not found")),
            SE_ERR_NOASSOC => {
                Failure::NotSupported(format!("no application is associated with {what}"))
            }
            SE_ERR_ACCESSDENIED => Failure::Operation(format!("Windows refused to open {what}")),
            other => {
                Failure::Operation(format!("ShellExecute failed with code {other} for {what}"))
            }
        })
    }

    pub(super) fn open_url(url: &str) -> Result<(), Failure> {
        shell_execute(std::ffi::OsStr::new(url), url)
    }

    pub(super) fn open_path(path: &Path) -> Result<(), Failure> {
        shell_execute(path.as_os_str(), &path.display().to_string())
    }

    /// Explorer with the item selected, through the API rather than a command
    /// line: `explorer /select,` parses its argument as text, and this does not.
    pub(super) fn show_item_in_folder(path: &Path) -> Result<(), Failure> {
        let wide_path = wide(path);
        in_apartment(|| {
            // SAFETY: the path is NUL-terminated and the returned list is freed
            // below on every route out.
            let item = unsafe { ILCreateFromPathW(wide_path.as_ptr()) };
            if item.is_null() {
                return Err(Failure::NotFound(format!(
                    "{} could not be resolved by the shell",
                    path.display()
                )));
            }
            // SAFETY: `item` is a valid list from `ILCreateFromPathW`; zero
            // children selects the item itself in its parent folder.
            let result = unsafe { SHOpenFolderAndSelectItems(item, 0, std::ptr::null(), 0) };
            // SAFETY: frees exactly the list created above.
            unsafe { ILFree(item) };
            if result >= 0 {
                Ok(())
            } else {
                Err(Failure::Operation(format!(
                    "Explorer could not show {} (HRESULT {result:#x})",
                    path.display()
                )))
            }
        })
    }
}

/// A `file:` URI for a local path, percent-encoded the way a file manager
/// expects to receive one.
#[cfg(target_os = "linux")]
fn file_uri(path: &Path) -> String {
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt;

    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(char::from(*byte));
            }
            _ => {
                let _ = write!(uri, "%{byte:02X}");
            }
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_and_mail_urls_are_handed_on_unchanged() {
        for url in [
            "https://github.com/krazyjakee/blitsen/issues/384",
            "HTTP://example.com",
            "http://localhost:8080/path?query=1#fragment",
            "mailto:someone@example.com?subject=Hi",
            "https://例え.jp/",
        ] {
            assert_eq!(external_url(url).as_deref(), Ok(url), "{url}");
        }
    }

    #[test]
    fn hostile_urls_are_refused_before_anything_is_spawned() {
        for (url, expected) in [
            ("", "must not be empty"),
            ("javascript:alert(1)", "javascript: is not a scheme"),
            ("file:///etc/passwd", "file: is not a scheme"),
            ("ftp://example.com/", "ftp: is not a scheme"),
            ("ms-settings:", "ms-settings: is not a scheme"),
            ("vscode://open", "vscode: is not a scheme"),
            ("example.com", "has no URL scheme"),
            ("/usr/bin/true", "has no URL scheme"),
            ("://", "has no URL scheme"),
            ("1http://example.com", "has no URL scheme"),
            ("https://", "names no host"),
            ("https:///path", "names no host"),
            ("http:example.com", "names no host"),
            ("mailto:", "names no address"),
            ("https://example.com/a b", "whitespace or control"),
            ("https://example.com/\n--evil", "whitespace or control"),
            ("https://example.com/\u{0}", "whitespace or control"),
            (" https://example.com", "whitespace or control"),
            ("https://example.com\t", "whitespace or control"),
        ] {
            let error = external_url(url).expect_err(url);
            assert!(
                error.message().contains(expected),
                "{url:?}: {} does not mention {expected:?}",
                error.message()
            );
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LENGTH));
        assert!(external_url(&long).is_err());
    }

    #[test]
    fn paths_are_absolute_and_carry_no_nul() {
        #[cfg(unix)]
        let absolute = "/tmp/blitsen file.txt";
        #[cfg(windows)]
        let absolute = r"C:\Users\blitsen\file.txt";
        assert_eq!(local_path(absolute), Ok(PathBuf::from(absolute)));
        for (path, expected) in [
            ("", "must not be empty"),
            ("relative/file.txt", "not an absolute path"),
            ("./file.txt", "not an absolute path"),
            ("~/file.txt", "not an absolute path"),
            ("-rf", "not an absolute path"),
            ("/tmp/\u{0}", "NUL byte"),
        ] {
            let error = local_path(path).expect_err(path);
            assert!(
                error.message().contains(expected),
                "{path:?}: {} does not mention {expected:?}",
                error.message()
            );
        }
    }

    #[test]
    fn a_missing_path_is_a_not_found_completion() {
        let missing =
            std::env::temp_dir().join(format!("blitsen-shell-{}-missing.txt", std::process::id()));
        let id = open_path(&missing.display().to_string()).expect("the request is accepted");
        assert!(pending());
        let mut completions = Vec::new();
        for _ in 0..400 {
            completions.extend(take());
            if !completions.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, id);
        let failure = completions[0]
            .outcome
            .clone()
            .expect_err("a missing path does not open");
        assert_eq!(failure.name(), "NotFoundError");
        assert!(failure.message().contains("missing.txt"));
        assert!(!pending());
    }

    #[test]
    fn failures_carry_the_dom_exception_name_they_are_raised_as() {
        assert_eq!(Failure::NotFound(String::new()).name(), "NotFoundError");
        assert_eq!(
            Failure::NotSupported(String::new()).name(),
            "NotSupportedError"
        );
        assert_eq!(Failure::Operation("x".into()).message(), "x");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uris_percent_encode_everything_a_file_manager_would_misread() {
        assert_eq!(
            file_uri(Path::new("/home/me/My Files/ré sumé#1.txt")),
            "file:///home/me/My%20Files/r%C3%A9%20sum%C3%A9%231.txt"
        );
        assert_eq!(
            file_uri(Path::new("/plain/path_1.txt")),
            "file:///plain/path_1.txt"
        );
    }
}

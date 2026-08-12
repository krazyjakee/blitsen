//! Application directories, single-instance ownership and relaunch.
//!
//! Deliberately not here: the command line, the executable path and exit.
//! Those are `process.argv`, `process.execPath` and `process.exit`, and
//! `native:` is additive rather than a second spelling of Node (TECH.md §9).

use std::path::PathBuf;
use std::process::Command;

use crate::PlatformError;

/// The per-application directory kinds every desktop platform defines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Directory {
    /// State that must survive a restart.
    Data,
    /// Recomputable state the system may delete at any time.
    Cache,
    /// User-editable configuration.
    Config,
}

/// Returns the platform directory of `kind` belonging to the application `name`.
///
/// The application states its own name because the runtime genuinely does not
/// know it: the executable is the host runtime during development, and a window
/// title is not an identity. The directory is returned, never created — making
/// it is `node:fs`.
pub fn directory(kind: Directory, name: &str) -> Result<PathBuf, PlatformError> {
    Ok(base_directory(kind)?.join(validated_name(name)?))
}

/// Accepts a name that is one path segment and nothing else, so a caller cannot
/// reach out of the directory the platform chose for it.
fn validated_name(name: &str) -> Result<&str, PlatformError> {
    let rejected = name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|character| character.is_control() || "/\\:".contains(character));
    if rejected {
        return Err(PlatformError::new(format!(
            "{name:?} is not a valid application name: one path segment, no separators"
        )));
    }
    Ok(name)
}

/// Returns `variable` when it names an absolute path, as XDG requires.
fn absolute_from_environment(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// The user's home directory, which every platform's answer is rooted in.
#[cfg(unix)]
fn home() -> Result<PathBuf, PlatformError> {
    absolute_from_environment("HOME").ok_or_else(|| {
        PlatformError::new("HOME names no absolute path, so there is no home directory")
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn base_directory(kind: Directory) -> Result<PathBuf, PlatformError> {
    let (variable, fallback) = match kind {
        Directory::Data => ("XDG_DATA_HOME", ".local/share"),
        Directory::Cache => ("XDG_CACHE_HOME", ".cache"),
        Directory::Config => ("XDG_CONFIG_HOME", ".config"),
    };
    match absolute_from_environment(variable) {
        Some(path) => Ok(path),
        None => Ok(home()?.join(fallback)),
    }
}

// Data and config resolve to the same directory, which is what macOS says:
// `~/Library/Preferences` is for the property lists `defaults` writes, not for
// an application's own configuration files.
#[cfg(target_os = "macos")]
fn base_directory(kind: Directory) -> Result<PathBuf, PlatformError> {
    let home = home()?;
    Ok(match kind {
        Directory::Data | Directory::Config => home.join("Library/Application Support"),
        Directory::Cache => home.join("Library/Caches"),
    })
}

#[cfg(windows)]
fn base_directory(kind: Directory) -> Result<PathBuf, PlatformError> {
    let variable = match kind {
        Directory::Data | Directory::Config => "APPDATA",
        Directory::Cache => "LOCALAPPDATA",
    };
    absolute_from_environment(variable).ok_or_else(|| {
        PlatformError::new(format!("{variable} names no absolute path on this system"))
    })
}

/// Spawns a fresh copy of this process with the same arguments, working
/// directory and environment, and releases any single-instance lock so the
/// successor can take it.
///
/// The caller stops itself afterwards with `process.exit`: shutting down is
/// Node's to spell, and only the application knows what it still has to flush.
pub fn relaunch() -> Result<(), PlatformError> {
    let executable = std::env::current_exe().map_err(|error| {
        PlatformError::new(format!("could not locate this executable: {error}"))
    })?;
    let mut command = Command::new(executable);
    command.args(std::env::args_os().skip(1));
    // The successor outlives this process, so it must not stay in a process
    // group the terminal is about to signal.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    single_instance::release();
    command
        .spawn()
        .map(drop)
        .map_err(|error| PlatformError::new(format!("could not start the successor: {error}")))
}

/// A second invocation of the application, as the primary instance sees it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Invocation {
    /// The second process's command line, exactly as it received it.
    pub argv: Vec<String>,
    /// Its working directory, so a relative path in `argv` still resolves.
    pub cwd: String,
}

/// Which side of the single-instance lock this process ended up on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Instance {
    /// This process owns the lock; second invocations arrive through
    /// [`single_instance::take`].
    Primary,
    /// Another process owns it, and this invocation was handed to that process.
    Secondary,
}

/// Single-instance ownership over a Unix domain socket.
///
/// The socket is both the lock and the hand-off channel: binding it is the
/// claim, and a process that cannot bind connects instead and posts its own
/// invocation to whoever did. Windows needs a named mutex and a pipe rather
/// than this, so the capability is absent there rather than approximated.
#[cfg(unix)]
pub mod single_instance {
    use std::io::{ErrorKind, Read, Write};
    use std::net::Shutdown;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    use super::{Instance, Invocation, validated_name};
    use crate::PlatformError;

    /// Invocations received but not yet read by the runtime.
    static RECEIVED: Mutex<Vec<Invocation>> = Mutex::new(Vec::new());
    /// Socket paths this process has bound, so [`release`] can unlink them.
    static BOUND: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    /// A peer that connects and then says nothing must not stall the hand-off.
    const READ_TIMEOUT: Duration = Duration::from_secs(5);

    fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims the lock for `name`, handing `invocation` to the owner if another
    /// process already holds it.
    pub fn request(name: &str, invocation: &Invocation) -> Result<Instance, PlatformError> {
        let path = socket_path(name)?;
        match UnixListener::bind(&path) {
            Ok(listener) => serve(listener, path),
            Err(error) if error.kind() == ErrorKind::AddrInUse => {
                match UnixStream::connect(&path) {
                    Ok(stream) => hand_over(stream, invocation),
                    // Nothing is listening: the socket outlived the process that
                    // made it, so the lock is ours to take.
                    Err(_) => {
                        remove(&path);
                        let listener = UnixListener::bind(&path).map_err(|error| {
                            PlatformError::new(format!(
                                "could not claim {}: {error}",
                                path.display()
                            ))
                        })?;
                        serve(listener, path)
                    }
                }
            }
            Err(error) => Err(PlatformError::new(format!(
                "could not claim {}: {error}",
                path.display()
            ))),
        }
    }

    /// Drains the invocations received since the last call.
    pub fn take() -> Vec<Invocation> {
        std::mem::take(&mut *locked(&RECEIVED))
    }

    /// Whether any invocation is waiting to be read.
    pub fn pending() -> bool {
        !locked(&RECEIVED).is_empty()
    }

    /// Unlinks every socket this process bound, so a successor can bind them.
    ///
    /// The listener itself is left open on the now-nameless inode: the caller is
    /// on its way out, and closing it would mean racing the accept thread.
    pub fn release() {
        for path in std::mem::take(&mut *locked(&BOUND)) {
            remove(&path);
        }
    }

    fn remove(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    /// Names the socket in the per-user runtime directory when there is one.
    ///
    /// The temporary-directory fallback is shared between users on Linux, which
    /// is why it is a fallback: it is the development path, and `XDG_RUNTIME_DIR`
    /// is set in every session that has a desktop in it.
    fn socket_path(name: &str) -> Result<PathBuf, PlatformError> {
        let name = validated_name(name)?;
        let path = match super::absolute_from_environment("XDG_RUNTIME_DIR") {
            Some(runtime) => runtime.join(format!("blitsen-{name}.sock")),
            None => {
                let user = std::env::var("USER")
                    .or_else(|_| std::env::var("LOGNAME"))
                    .unwrap_or_else(|_| "anonymous".to_owned());
                let user = user.replace(['/', '\\', ':'], "-");
                std::env::temp_dir().join(format!("blitsen-{user}-{name}.sock"))
            }
        };
        // `sun_path` is 104 bytes on macOS and 108 on Linux; a name that does not
        // fit has to fail here rather than inside `bind`.
        if path.as_os_str().len() >= 100 {
            return Err(PlatformError::new(format!(
                "the single-instance socket path is too long: {}",
                path.display()
            )));
        }
        Ok(path)
    }

    fn serve(listener: UnixListener, path: PathBuf) -> Result<Instance, PlatformError> {
        locked(&BOUND).push(path);
        std::thread::Builder::new()
            .name("blitsen-single-instance".to_owned())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    if let Some(invocation) = receive(stream) {
                        locked(&RECEIVED).push(invocation);
                    }
                }
            })
            .map_err(|error| {
                PlatformError::new(format!(
                    "could not start the single-instance listener: {error}"
                ))
            })?;
        Ok(Instance::Primary)
    }

    fn receive(mut stream: UnixStream) -> Option<Invocation> {
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        let mut payload = String::new();
        stream.read_to_string(&mut payload).ok()?;
        serde_json::from_str(&payload).ok()
    }

    fn hand_over(
        mut stream: UnixStream,
        invocation: &Invocation,
    ) -> Result<Instance, PlatformError> {
        let payload = serde_json::to_vec(invocation).map_err(|error| {
            PlatformError::new(format!("could not encode this invocation: {error}"))
        })?;
        stream
            .write_all(&payload)
            .and_then(|()| stream.flush())
            .and_then(|()| stream.shutdown(Shutdown::Write))
            .map_err(|error| {
                PlatformError::new(format!("could not hand this invocation over: {error}"))
            })?;
        Ok(Instance::Secondary)
    }
}

#[cfg(not(unix))]
mod single_instance {
    /// Nothing to unlink where there is no socket to bind.
    pub(super) fn release() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_names_are_one_path_segment() {
        assert!(validated_name("My App").is_ok());
        for rejected in ["", ".", "..", "a/b", "a\\b", "c:name", "line\nbreak"] {
            assert!(
                validated_name(rejected).is_err(),
                "{rejected:?} must be rejected"
            );
        }
    }

    #[test]
    fn directories_are_absolute_and_end_with_the_application_name() {
        for kind in [Directory::Data, Directory::Cache, Directory::Config] {
            let path = directory(kind, "blitsen-unit-test").expect("a home directory");
            assert!(path.is_absolute(), "{path:?} must be absolute");
            assert_eq!(
                path.file_name().and_then(|name| name.to_str()),
                Some("blitsen-unit-test")
            );
        }
    }

    #[test]
    fn cache_is_a_different_directory_from_data() {
        let data = directory(Directory::Data, "blitsen-unit-test").expect("a home directory");
        let cache = directory(Directory::Cache, "blitsen-unit-test").expect("a home directory");
        assert_ne!(data, cache);
    }
}

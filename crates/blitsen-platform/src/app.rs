//! Application directories, single-instance ownership and relaunch.
//!
//! Deliberately not here: the command line, the executable path and exit.
//! The shipped runtime exposes no Node process surface; the focused `blitsen/app`
//! module deliberately does not invent equivalents for them (TECH.md §9).
//!
//! Absent on Android, and each of the three capabilities for its own reason
//! rather than one blanket one — which is the point of writing them down, since
//! they come back at different times.
//!
//! * **The directories** are the Activity's `filesDir` and `cacheDir`, and only
//!   the Activity can name them. The desktop directory provider cannot discover
//!   that Activity-owned location and may fall through to a plausible-looking
//!   but unwritable home path. That is the shape `docs/PRODUCT.md` §7 exists to
//!   refuse. There is also no third
//!   kind: Android's configuration is `SharedPreferences`, a store rather than a
//!   place, so `Directory::Config` has no honest answer even once the other two
//!   arrive.
//! * **[`relaunch`]** spawns `current_exe()` with this process's own argument
//!   list. Inside an APK there is no executable to spawn — the code is a shared
//!   object the zygote loaded — and restarting is an `Intent` handed to the
//!   system, not a child this process forks and outlives.
//! * **[`single_instance`]** is the platform's job already on Android. An Android
//!   application is one process by construction, a second launch is delivered to
//!   the instance running rather than started beside it, and what arrives is an
//!   `Intent` rather than an `argv` and a working directory. Binding a socket to
//!   win a race nothing is racing would be ceremony, and [`Invocation`] has no
//!   fields an `Intent` fills.
//!
//! The first two are reachable through JNI once there is an entry point holding
//! an `AndroidApp` (#142); the third stays absent because it is answering a
//! question Android does not ask (#147).

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

/// Returns an absolute Linux runtime endpoint directory named by the environment.
///
/// This is deliberately limited to the single-instance endpoint below;
/// persistent application directories come from the platform provider.
#[cfg(target_os = "linux")]
fn absolute_from_environment(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn base_directory(kind: Directory) -> Result<PathBuf, PlatformError> {
    let directory = match kind {
        Directory::Data => dirs::data_dir(),
        Directory::Cache => dirs::cache_dir(),
        Directory::Config => dirs::config_dir(),
    };
    directory.ok_or_else(|| {
        PlatformError::new(format!(
            "the operating system did not provide a {} directory",
            match kind {
                Directory::Data => "data",
                Directory::Cache => "cache",
                Directory::Config => "configuration",
            }
        ))
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

/// Single-instance ownership over a local byte stream.
///
/// Binding is both the ownership claim and the hand-off channel: Unix uses a
/// filesystem domain socket and Windows uses a named pipe. Blitsen retains the
/// Length-prefixed JSON framing, authenticated acknowledgement, queue and
/// listener lifecycle above that transport.
///
/// Unix endpoints live in a mode-0700 per-user runtime directory. Linux uses
/// `XDG_RUNTIME_DIR` when it is absolute, owned by the effective user and not
/// group/world accessible; the fallback is `<temp>/blitsen-<uid>`. macOS uses
/// `<per-user temp>/blitsen-ipc`; its short socket filename is a deterministic
/// hash of the full application name because Darwin's `sun_path` is only 104
/// bytes. Socket files are mode 0600 on Linux and the private parent is the
/// permission boundary on macOS, where pre-bind socket modes are unsupported.
/// A short advisory lock serializes stale-file recovery, but binding the socket
/// remains the persistent ownership claim.
///
/// Windows endpoints are discoverable as
/// `\\.\pipe\blitsen-<user SID>-<application>`. Their protected DACL grants only
/// that SID access once Blitsen owns the pipe. A different user can still race
/// to create a discoverable name first, so clients verify the connected pipe
/// object's owner SID before sending; servers impersonate each connected client
/// and verify its token SID. Processes under the same SID share this trust
/// boundary and can intentionally contend for the same application name.
#[cfg(not(target_os = "android"))]
pub mod single_instance;

#[cfg(target_os = "android")]
mod single_instance {
    /// Android owns second-launch delivery, so there is no endpoint to release.
    pub(super) fn release() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "ios"))
    ))]
    const DIRECTORY_CHILD: &str = "BLITSEN_DIRECTORY_PROVIDER_CHILD";

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

    // Environment-sensitive XDG cases run in child test processes. Mutating
    // HOME/XDG variables in this process would race the test runner's threads.
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "ios"))
    ))]
    #[test]
    fn xdg_directories_honor_absolute_values_and_reject_relative_ones() {
        let executable = std::env::current_exe().expect("the test executable has a path");
        for scenario in ["absolute", "fallback"] {
            let mut child = std::process::Command::new(&executable);
            child
                .args([
                    "--exact",
                    "app::tests::xdg_directory_provider_child",
                    "--ignored",
                ])
                .env(DIRECTORY_CHILD, scenario)
                .env("HOME", "/tmp/blitsen-dirs-home")
                .env(
                    "XDG_DATA_HOME",
                    if scenario == "absolute" {
                        "/tmp/blitsen-dirs-data"
                    } else {
                        "relative-is-invalid"
                    },
                );
            let output = child.output().expect("the child test starts");
            assert!(
                output.status.success(),
                "{scenario} child failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "ios"))
    ))]
    #[test]
    #[ignore = "invoked hermetically by xdg_directories_honor_absolute_values_and_reject_relative_ones"]
    fn xdg_directory_provider_child() {
        let expected = match std::env::var(DIRECTORY_CHILD).as_deref() {
            Ok("absolute") => PathBuf::from("/tmp/blitsen-dirs-data"),
            Ok("fallback") => PathBuf::from("/tmp/blitsen-dirs-home/.local/share"),
            scenario => panic!("unexpected child scenario: {scenario:?}"),
        };
        assert_eq!(base_directory(Directory::Data).unwrap(), expected);
    }
}

//! Application directories, single-instance ownership and relaunch.
//!
//! Deliberately not here: the command line, the executable path and exit.
//! Those are `process.argv`, `process.execPath` and `process.exit`, and
//! `native:` is additive rather than a second spelling of Node (TECH.md §9).
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
pub mod single_instance {
    use std::io::{ErrorKind, Read, Write};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};
    #[cfg(unix)]
    use std::{fs::File, path::PathBuf};

    #[cfg(unix)]
    use interprocess::local_socket::GenericFilePath;
    #[cfg(windows)]
    use interprocess::local_socket::GenericNamespaced;
    use interprocess::local_socket::prelude::*;
    use interprocess::local_socket::{ListenerNonblockingMode, ListenerOptions, Name};
    use parking_lot::Mutex;

    use super::{Instance, Invocation, validated_name};
    use crate::PlatformError;

    /// Invocations received but not yet read by the runtime. A malformed peer
    /// or panicking hand-off thread must not poison later invocations.
    static RECEIVED: Mutex<Vec<Invocation>> = Mutex::new(Vec::new());
    /// Listener threads owned by this process. Stopping and joining them drops
    /// the transport, reclaiming a Unix name or closing the Windows pipe.
    static SERVERS: Mutex<Vec<Server>> = Mutex::new(Vec::new());

    /// A peer that connects and then says nothing must not stall the hand-off.
    #[cfg(not(test))]
    const READ_TIMEOUT: Duration = Duration::from_secs(5);
    #[cfg(test)]
    const READ_TIMEOUT: Duration = Duration::from_millis(250);
    const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
    const POLL_INTERVAL: Duration = Duration::from_millis(5);
    const MAX_INVOCATION_BYTES: usize = 1024 * 1024;
    const ACKNOWLEDGEMENT: u8 = 1;
    const ACKNOWLEDGEMENT_RECEIPT: u8 = 2;

    struct Endpoint {
        name: Name<'static>,
        #[cfg(unix)]
        path: PathBuf,
        #[cfg(windows)]
        user_sid: String,
    }

    struct Server {
        stop: Arc<AtomicBool>,
        thread: JoinHandle<()>,
    }

    /// Claims the lock for `name`, handing `invocation` to the owner if another
    /// process already holds it.
    pub fn request(name: &str, invocation: &Invocation) -> Result<Instance, PlatformError> {
        let endpoint = endpoint(name)?;
        #[cfg(unix)]
        let election = election_lock(&endpoint)?;
        match bind(&endpoint) {
            Ok(listener) => serve(listener),
            Err(error) if endpoint_is_occupied(&error) => {
                match connect(&endpoint) {
                    Ok(stream) => {
                        #[cfg(unix)]
                        drop(election);
                        hand_over(stream, invocation)
                    }
                    #[cfg(unix)]
                    // Nothing is listening: the socket outlived the process that
                    // made it. The election lock prevents two recovering
                    // processes from both unlinking a successor's live socket.
                    Err(error) if unix_endpoint_is_stale(&error) => {
                        let _ = std::fs::remove_file(&endpoint.path);
                        let listener = bind(&endpoint).map_err(|error| {
                            PlatformError::new(format!(
                                "could not claim {}: {error}",
                                endpoint.path.display()
                            ))
                        })?;
                        serve(listener)
                    }
                    Err(error) => Err(PlatformError::new(format!(
                        "could not connect to the single-instance owner: {error}"
                    ))),
                }
            }
            Err(error) => Err(PlatformError::new(format!(
                "could not claim the single-instance endpoint: {error}"
            ))),
        }
    }

    fn endpoint_is_occupied(error: &std::io::Error) -> bool {
        error.kind() == ErrorKind::AddrInUse
            // FILE_FLAG_FIRST_PIPE_INSTANCE reports ERROR_ACCESS_DENIED when
            // another server owns the discoverable pipe name.
            || cfg!(windows) && error.kind() == ErrorKind::PermissionDenied
    }

    #[cfg(unix)]
    fn unix_endpoint_is_stale(error: &std::io::Error) -> bool {
        // These are the only connect results that prove there is no listener.
        // Permission, timeout, interruption and resource errors may be
        // transient or an authentication boundary and must never trigger an
        // unlink of a possibly live owner's endpoint.
        matches!(
            error.kind(),
            ErrorKind::ConnectionRefused | ErrorKind::NotFound
        )
    }

    /// Drains the invocations received since the last call.
    pub fn take() -> Vec<Invocation> {
        std::mem::take(&mut *RECEIVED.lock())
    }

    /// Whether any invocation is waiting to be read.
    pub fn pending() -> bool {
        !RECEIVED.lock().is_empty()
    }

    /// Stops every listener this process started and releases its endpoint.
    pub fn release() {
        let servers = std::mem::take(&mut *SERVERS.lock());
        for server in &servers {
            server.stop.store(true, Ordering::Release);
        }
        for server in servers {
            let _ = server.thread.join();
        }
    }

    #[cfg(unix)]
    fn endpoint(name: &str) -> Result<Endpoint, PlatformError> {
        let name = validated_name(name)?;
        #[cfg(target_os = "macos")]
        let filename = format!("b-{:016x}.sock", stable_name_hash(&name));
        #[cfg(not(target_os = "macos"))]
        let filename = format!("blitsen-{name}.sock");
        let path = unix_runtime_directory()?.join(filename);
        // `sun_path` is 104 bytes on macOS and 108 on Linux; a name that does not
        // fit has to fail here rather than inside `bind`.
        if path.as_os_str().len() >= 100 {
            return Err(PlatformError::new(format!(
                "the single-instance socket path is too long: {}",
                path.display()
            )));
        }
        let socket_name = path
            .clone()
            .to_fs_name::<GenericFilePath>()
            .map_err(|error| {
                PlatformError::new(format!("could not name {}: {error}", path.display()))
            })?
            .into_owned();
        Ok(Endpoint {
            name: socket_name,
            path,
        })
    }

    #[cfg(target_os = "macos")]
    fn stable_name_hash(name: &str) -> u64 {
        // FNV-1a keeps this endpoint stable across processes and toolchains
        // without adding a shipping dependency. The full validated name feeds
        // all 64 bits; this is an identity key, not an authenticity boundary.
        name.as_bytes()
            .iter()
            .fold(0xcbf29ce484222325, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            })
    }

    #[cfg(windows)]
    fn endpoint(name: &str) -> Result<Endpoint, PlatformError> {
        let name = validated_name(name)?;
        let user_sid = windows::current_user_sid()?;
        let pipe_name = format!("blitsen-{user_sid}-{name}");
        let socket_name = pipe_name
            .to_ns_name::<GenericNamespaced>()
            .map_err(|error| {
                PlatformError::new(format!("could not name the Windows pipe: {error}"))
            })?
            .into_owned();
        Ok(Endpoint {
            name: socket_name,
            user_sid,
        })
    }

    #[cfg(unix)]
    fn unix_runtime_directory() -> Result<PathBuf, PlatformError> {
        #[cfg(target_os = "linux")]
        if let Some(path) = super::absolute_from_environment("XDG_RUNTIME_DIR") {
            validate_private_directory(&path)?;
            return Ok(path);
        }
        let path = if cfg!(target_os = "macos") {
            std::env::temp_dir().join("blitsen-ipc")
        } else {
            std::env::temp_dir().join(format!("blitsen-{}", unsafe { libc::geteuid() }))
        };
        create_private_directory(&path)?;
        Ok(path)
    }

    #[cfg(unix)]
    fn create_private_directory(path: &std::path::Path) -> Result<(), PlatformError> {
        use std::os::unix::fs::DirBuilderExt;

        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(PlatformError::new(format!(
                    "could not create the single-instance directory {}: {error}",
                    path.display()
                )));
            }
        }
        validate_private_directory(path)
    }

    #[cfg(unix)]
    fn validate_private_directory(path: &std::path::Path) -> Result<(), PlatformError> {
        use std::os::unix::fs::MetadataExt;

        let metadata = std::fs::symlink_metadata(path).map_err(|error| {
            PlatformError::new(format!(
                "could not inspect the single-instance directory {}: {error}",
                path.display()
            ))
        })?;
        let private = metadata.file_type().is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0;
        if !private {
            return Err(PlatformError::new(format!(
                "the single-instance directory must be owned by this user with mode 0700: {}",
                path.display()
            )));
        }
        Ok(())
    }

    #[cfg(unix)]
    fn election_lock(endpoint: &Endpoint) -> Result<File, PlatformError> {
        use std::os::unix::fs::OpenOptionsExt;

        let lock_path = endpoint.path.with_extension("lock");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)
            .map_err(|error| {
                PlatformError::new(format!(
                    "could not open the single-instance election {}: {error}",
                    lock_path.display()
                ))
            })?;
        file.lock().map_err(|error| {
            PlatformError::new(format!(
                "could not lock the single-instance election {}: {error}",
                lock_path.display()
            ))
        })?;
        Ok(file)
    }

    fn bind(endpoint: &Endpoint) -> std::io::Result<LocalSocketListener> {
        let options = ListenerOptions::new()
            .name(endpoint.name.borrow())
            .nonblocking(ListenerNonblockingMode::Both)
            .reclaim_name(true);
        #[cfg(target_os = "linux")]
        let options = {
            use interprocess::os::unix::local_socket::ListenerOptionsExt;
            options.mode(0o600)
        };
        #[cfg(windows)]
        let options = {
            use interprocess::os::windows::local_socket::ListenerOptionsExt;
            options.security_descriptor(windows::security_descriptor(&endpoint.user_sid)?)
        };
        options.create_sync()
    }

    fn connect(endpoint: &Endpoint) -> std::io::Result<LocalSocketStream> {
        let stream = LocalSocketStream::connect(endpoint.name.borrow())?;
        authenticate_peer(&stream, endpoint)?;
        stream.set_nonblocking(true)?;
        Ok(stream)
    }

    fn authenticate_peer(stream: &LocalSocketStream, _endpoint: &Endpoint) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            let credentials = stream.peer_creds()?;
            if credentials.euid() != Some(unsafe { libc::geteuid() }) {
                return Err(std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    "the single-instance peer belongs to another user",
                ));
            }
        }
        #[cfg(windows)]
        windows::authenticate_server(stream, &_endpoint.user_sid)?;
        Ok(())
    }

    fn serve(listener: LocalSocketListener) -> Result<Instance, PlatformError> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("blitsen-single-instance".to_owned())
            .spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok(mut stream) => {
                            match receive(&mut stream) {
                                Ok(invocation) => {
                                    RECEIVED.lock().push(invocation);
                                    // Keep a real secondary alive until Windows
                                    // has authenticated its connected pipe token
                                    // and the invocation is durably owned here.
                                    if let Err(error) =
                                        write_acknowledgement(&mut stream, WRITE_TIMEOUT)
                                    {
                                        eprintln!(
                                            "could not acknowledge single-instance hand-off: {error}"
                                        );
                                    } else if let Err(error) =
                                        read_acknowledgement_receipt(&mut stream, READ_TIMEOUT)
                                    {
                                        eprintln!(
                                            "single-instance acknowledgement was not received: {error}"
                                        );
                                    }
                                }
                                Err(error) => {
                                    eprintln!("rejected single-instance hand-off: {error}");
                                }
                            }
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            std::thread::sleep(POLL_INTERVAL);
                        }
                        Err(error) if error.kind() == ErrorKind::Interrupted => {}
                        Err(_) => std::thread::sleep(POLL_INTERVAL),
                    }
                }
            })
            .map_err(|error| {
                PlatformError::new(format!(
                    "could not start the single-instance listener: {error}"
                ))
            })?;
        SERVERS.lock().push(Server { stop, thread });
        Ok(Instance::Primary)
    }

    fn authenticate_client(stream: &LocalSocketStream) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            let credentials = stream.peer_creds()?;
            if credentials.euid() != Some(unsafe { libc::geteuid() }) {
                return Err(std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    "the single-instance client belongs to another user",
                ));
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            let sid = windows::current_user_sid()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            windows::authenticate_client(stream, &sid)
        }
    }

    fn receive(stream: &mut LocalSocketStream) -> Result<Invocation, String> {
        #[cfg(unix)]
        authenticate_client(stream).map_err(|error| error.to_string())?;

        let deadline = Instant::now() + READ_TIMEOUT;
        let mut encoded_size = [0; std::mem::size_of::<u32>()];
        read_stream_exact_until(stream, &mut encoded_size, deadline)
            .map_err(|error| error.to_string())?;
        let size = decoded_size(encoded_size).map_err(|error| error.to_string())?;

        // Impersonation uses the security context of the last bytes read from
        // this pipe. Authenticate immediately after the length prefix, while
        // the acknowledged client is necessarily still connected, rather than
        // after a final read that may observe pipe teardown.
        #[cfg(windows)]
        authenticate_client(stream).map_err(|error| error.to_string())?;

        let mut payload = vec![0; size];
        read_stream_exact_until(stream, &mut payload, deadline)
            .map_err(|error| error.to_string())?;
        serde_json::from_slice(&payload).map_err(|error| error.to_string())
    }

    #[cfg(test)]
    fn read_payload(reader: &mut impl Read, timeout: Duration) -> std::io::Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut encoded_size = [0; std::mem::size_of::<u32>()];
        read_exact_until(reader, &mut encoded_size, deadline)?;
        let size = decoded_size(encoded_size)?;
        let mut payload = vec![0; size];
        read_exact_until(reader, &mut payload, deadline)?;
        Ok(payload)
    }

    fn decoded_size(encoded_size: [u8; 4]) -> std::io::Result<usize> {
        let size = u32::from_be_bytes(encoded_size) as usize;
        if size > MAX_INVOCATION_BYTES {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "the single-instance invocation frame is too large",
            ));
        }
        Ok(size)
    }

    fn hand_over(
        mut stream: LocalSocketStream,
        invocation: &Invocation,
    ) -> Result<Instance, PlatformError> {
        let payload = serde_json::to_vec(invocation).map_err(|error| {
            PlatformError::new(format!("could not encode this invocation: {error}"))
        })?;
        write_payload(&mut stream, &payload, WRITE_TIMEOUT).map_err(|error| {
            PlatformError::new(format!("could not hand this invocation over: {error}"))
        })?;
        read_acknowledgement(&mut stream, WRITE_TIMEOUT).map_err(|error| {
            PlatformError::new(format!(
                "the single-instance owner did not acknowledge this invocation: {error}"
            ))
        })?;
        write_acknowledgement_receipt(&mut stream, WRITE_TIMEOUT).map_err(|error| {
            PlatformError::new(format!(
                "could not confirm the single-instance acknowledgement: {error}"
            ))
        })?;
        Ok(Instance::Secondary)
    }

    fn write_payload(
        writer: &mut impl Write,
        payload: &[u8],
        timeout: Duration,
    ) -> std::io::Result<()> {
        if payload.len() > MAX_INVOCATION_BYTES {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "the single-instance invocation frame is too large",
            ));
        }
        let encoded_size = (payload.len() as u32).to_be_bytes();
        let deadline = Instant::now() + timeout;
        write_all_until(writer, &encoded_size, deadline)?;
        write_all_until(writer, payload, deadline)?;
        writer.flush()
    }

    fn read_acknowledgement(
        reader: &mut LocalSocketStream,
        timeout: Duration,
    ) -> std::io::Result<()> {
        let mut acknowledgement = [0];
        read_stream_exact_until(reader, &mut acknowledgement, Instant::now() + timeout)?;
        if acknowledgement != [ACKNOWLEDGEMENT] {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "the single-instance owner sent an invalid acknowledgement",
            ));
        }
        Ok(())
    }

    fn write_acknowledgement(writer: &mut impl Write, timeout: Duration) -> std::io::Result<()> {
        write_all_until(writer, &[ACKNOWLEDGEMENT], Instant::now() + timeout)?;
        writer.flush()
    }

    fn read_acknowledgement_receipt(
        reader: &mut LocalSocketStream,
        timeout: Duration,
    ) -> std::io::Result<()> {
        let mut receipt = [0];
        read_stream_exact_until(reader, &mut receipt, Instant::now() + timeout)?;
        if receipt != [ACKNOWLEDGEMENT_RECEIPT] {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "the single-instance client sent an invalid acknowledgement receipt",
            ));
        }
        Ok(())
    }

    fn write_acknowledgement_receipt(
        writer: &mut impl Write,
        timeout: Duration,
    ) -> std::io::Result<()> {
        write_all_until(writer, &[ACKNOWLEDGEMENT_RECEIPT], Instant::now() + timeout)?;
        writer.flush()
    }

    #[cfg(any(unix, test))]
    fn read_exact_until(
        reader: &mut impl Read,
        buffer: &mut [u8],
        deadline: Instant,
    ) -> std::io::Result<()> {
        let mut read = 0;
        while read < buffer.len() {
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    ErrorKind::TimedOut,
                    "the single-instance invocation frame timed out",
                ));
            }
            match reader.read(&mut buffer[read..]) {
                Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
                Ok(count) => read += count,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn read_stream_exact_until(
        stream: &mut LocalSocketStream,
        buffer: &mut [u8],
        deadline: Instant,
    ) -> std::io::Result<()> {
        #[cfg(unix)]
        return read_exact_until(stream, buffer, deadline);

        #[cfg(windows)]
        {
            let mut read = 0;
            while read < buffer.len() {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        "the single-instance invocation frame timed out",
                    ));
                }
                let available = windows::bytes_available(stream)?;
                if available == 0 {
                    std::thread::sleep(POLL_INTERVAL);
                    continue;
                }
                let end = read + available.min(buffer.len() - read);
                match stream.read(&mut buffer[read..end]) {
                    Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
                    Ok(count) => read += count,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(POLL_INTERVAL);
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        }
    }

    fn write_all_until(
        writer: &mut impl Write,
        payload: &[u8],
        deadline: Instant,
    ) -> std::io::Result<()> {
        let mut written = 0;
        while written < payload.len() {
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    ErrorKind::TimedOut,
                    "the single-instance invocation hand-off timed out",
                ));
            }
            match writer.write(&payload[written..]) {
                Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
                Ok(count) => written += count,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use std::io::{Cursor, Read, Write};
        use std::sync::Barrier;
        use std::sync::atomic::{AtomicU64, Ordering};

        use parking_lot::Mutex;

        use super::*;

        static SERIAL: Mutex<()> = Mutex::new(());
        static NEXT_NAME: AtomicU64 = AtomicU64::new(1);
        const CHILD_NAME: &str = "BLITSEN_SINGLE_INSTANCE_CHILD_NAME";

        fn unique_name(case: &str) -> String {
            format!(
                "ipc-{case}-{}-{}",
                std::process::id(),
                NEXT_NAME.fetch_add(1, Ordering::Relaxed)
            )
        }

        fn invocation(id: usize) -> Invocation {
            Invocation {
                argv: vec!["blitsen".to_owned(), format!("document-{id}.html")],
                cwd: format!("/working/{id}"),
            }
        }

        fn wait_for_received(expected: usize) -> Vec<Invocation> {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut received = Vec::new();
            while received.len() < expected && Instant::now() < deadline {
                received.extend(take());
                std::thread::sleep(POLL_INTERVAL);
            }
            assert_eq!(received.len(), expected);
            received
        }

        fn reset() {
            release();
            take();
        }

        #[test]
        fn invocation_frames_are_bounded_in_size_and_wall_clock_time() {
            let oversized = vec![b'x'; MAX_INVOCATION_BYTES + 1];
            let error =
                read_payload(&mut Cursor::new(&oversized), Duration::from_secs(1)).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidData);

            let error =
                write_payload(&mut Vec::new(), &oversized, Duration::from_secs(1)).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidInput);

            struct Drip {
                offset: usize,
            }
            impl Read for Drip {
                fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                    std::thread::sleep(Duration::from_millis(2));
                    let frame = [0, 0, 0, 100];
                    buffer[0] = frame.get(self.offset).copied().unwrap_or(b' ');
                    self.offset += 1;
                    Ok(1)
                }
            }
            let started = Instant::now();
            let error =
                read_payload(&mut Drip { offset: 0 }, Duration::from_millis(20)).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::TimedOut);
            assert!(started.elapsed() < Duration::from_millis(200));

            struct Blocked;
            impl Write for Blocked {
                fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
                    Err(ErrorKind::WouldBlock.into())
                }

                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let started = Instant::now();
            let error = write_payload(
                &mut Blocked,
                br#"{"argv":[],"cwd":"/"}"#,
                Duration::from_millis(20),
            )
            .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::TimedOut);
            assert!(started.elapsed() < Duration::from_millis(200));
        }

        #[cfg(unix)]
        #[test]
        fn only_errors_proving_no_unix_listener_allow_stale_recovery() {
            for kind in [ErrorKind::ConnectionRefused, ErrorKind::NotFound] {
                assert!(unix_endpoint_is_stale(&std::io::Error::from(kind)));
            }
            for kind in [
                ErrorKind::PermissionDenied,
                ErrorKind::TimedOut,
                ErrorKind::WouldBlock,
                ErrorKind::Interrupted,
                ErrorKind::ConnectionReset,
            ] {
                assert!(!unix_endpoint_is_stale(&std::io::Error::from(kind)));
            }
        }

        #[test]
        fn a_secondary_transfers_the_complete_invocation() {
            let _serial = SERIAL.lock();
            reset();
            let name = unique_name("transfer");
            assert_eq!(request(&name, &invocation(0)).unwrap(), Instance::Primary);
            let sent = invocation(1);
            assert_eq!(request(&name, &sent).unwrap(), Instance::Secondary);
            assert_eq!(wait_for_received(1), vec![sent]);
            reset();
        }

        #[test]
        fn a_secondary_process_transfers_and_exits_cleanly() {
            let _serial = SERIAL.lock();
            reset();
            let name = unique_name("process");
            assert_eq!(request(&name, &invocation(0)).unwrap(), Instance::Primary);
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "app::single_instance::tests::secondary_process_child",
                    "--ignored",
                ])
                .env(CHILD_NAME, &name)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "secondary failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(wait_for_received(1), vec![invocation(99)]);
            reset();
        }

        #[test]
        #[ignore = "invoked in a separate process by a_secondary_process_transfers_and_exits_cleanly"]
        fn secondary_process_child() {
            let name = std::env::var(CHILD_NAME).expect("the parent supplies an endpoint name");
            assert_eq!(
                request(&name, &invocation(99)).unwrap(),
                Instance::Secondary
            );
        }

        #[test]
        fn concurrent_requests_elect_exactly_one_owner() {
            let _serial = SERIAL.lock();
            reset();
            let name = unique_name("election");
            let contenders = 8;
            let barrier = Arc::new(Barrier::new(contenders));
            let threads = (0..contenders)
                .map(|id| {
                    let barrier = Arc::clone(&barrier);
                    let name = name.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        request(&name, &invocation(id)).unwrap()
                    })
                })
                .collect::<Vec<_>>();
            let outcomes = threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|outcome| **outcome == Instance::Primary)
                    .count(),
                1
            );
            wait_for_received(contenders - 1);
            reset();
        }

        #[cfg(unix)]
        #[test]
        fn a_crashed_unix_listener_is_reclaimed_under_the_election_lock() {
            let _serial = SERIAL.lock();
            reset();
            let name = unique_name("stale");
            let endpoint = endpoint(&name).unwrap();
            let mut stale = bind(&endpoint).unwrap();
            stale.do_not_reclaim_name_on_drop();
            drop(stale);
            assert!(endpoint.path.exists());
            assert_eq!(request(&name, &invocation(0)).unwrap(), Instance::Primary);
            reset();
            assert!(!endpoint.path.exists());
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn macos_hashes_long_application_names_below_the_socket_limit() {
            let first = endpoint(&"a".repeat(80)).unwrap();
            let second = endpoint(&format!("{}b", "a".repeat(79))).unwrap();
            assert!(first.path.as_os_str().len() < 100);
            assert_ne!(first.path, second.path);
        }

        #[cfg(windows)]
        #[test]
        fn a_closed_named_pipe_leaves_no_stale_endpoint() {
            let _serial = SERIAL.lock();
            reset();
            let endpoint = endpoint(&unique_name("stale")).unwrap();
            drop(bind(&endpoint).unwrap());
            drop(bind(&endpoint).expect("the pipe name dies with its final handle"));
        }

        #[test]
        fn release_stops_the_listener_before_the_name_is_reused() {
            let _serial = SERIAL.lock();
            reset();
            let name = unique_name("release");
            let endpoint = endpoint(&name).unwrap();
            assert_eq!(request(&name, &invocation(0)).unwrap(), Instance::Primary);

            // A silent peer parks the listener in the frame reader. Its fixed
            // deadline must still let release join the thread promptly.
            let silent = LocalSocketStream::connect(endpoint.name.borrow()).unwrap();
            std::thread::sleep(POLL_INTERVAL * 4);
            let started = Instant::now();
            release();
            assert!(started.elapsed() < Duration::from_secs(1));
            drop(silent);

            assert_eq!(request(&name, &invocation(1)).unwrap(), Instance::Primary);
            reset();
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn linux_endpoint_and_parent_are_private() {
            use std::os::unix::fs::MetadataExt;

            let _serial = SERIAL.lock();
            reset();
            let name = unique_name("permissions");
            let endpoint = endpoint(&name).unwrap();
            assert_eq!(request(&name, &invocation(0)).unwrap(), Instance::Primary);
            assert_eq!(
                std::fs::metadata(&endpoint.path).unwrap().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(endpoint.path.parent().unwrap())
                    .unwrap()
                    .mode()
                    & 0o077,
                0
            );
            reset();
        }

        #[cfg(windows)]
        #[test]
        fn windows_pipe_name_dacl_owner_and_client_are_scoped_to_the_user_sid() {
            let _serial = SERIAL.lock();
            let endpoint = endpoint(&unique_name("security")).unwrap();
            assert!(endpoint.name.is_namespaced());
            assert!(endpoint.user_sid.starts_with("S-1-"));
            windows::security_descriptor(&endpoint.user_sid).unwrap();

            let listener = bind(&endpoint).unwrap();
            let mut client = LocalSocketStream::connect(endpoint.name.borrow()).unwrap();
            windows::authenticate_server(&client, &endpoint.user_sid).unwrap();
            assert_eq!(
                windows::authenticate_server(&client, "S-1-0-0")
                    .unwrap_err()
                    .kind(),
                ErrorKind::PermissionDenied
            );
            let mut server = loop {
                match listener.accept() {
                    Ok(stream) => break stream,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(POLL_INTERVAL);
                    }
                    Err(error) => panic!("could not accept the test pipe: {error}"),
                }
            };
            write_payload(
                &mut client,
                br#"{"argv":[],"cwd":"C:\\"}"#,
                Duration::from_secs(1),
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(1);
            let mut encoded_size = [0; std::mem::size_of::<u32>()];
            read_exact_until(&mut server, &mut encoded_size, deadline).unwrap();
            let size = decoded_size(encoded_size).unwrap();
            windows::authenticate_client(&server, &endpoint.user_sid).unwrap();
            assert_eq!(
                windows::authenticate_client(&server, "S-1-0-0")
                    .unwrap_err()
                    .kind(),
                ErrorKind::PermissionDenied
            );
            let mut payload = vec![0; size];
            read_exact_until(&mut server, &mut payload, deadline).unwrap();
            assert_eq!(payload, br#"{"argv":[],"cwd":"C:\\"}"#);
            write_acknowledgement(&mut server, Duration::from_secs(1)).unwrap();
            read_acknowledgement(&mut client, Duration::from_secs(1)).unwrap();
            write_acknowledgement_receipt(&mut client, Duration::from_secs(1)).unwrap();
            read_acknowledgement_receipt(&mut server, Duration::from_secs(1)).unwrap();
        }
    }

    #[cfg(windows)]
    mod windows {
        use std::ffi::c_void;
        use std::io;
        use std::mem::{align_of, size_of};
        use std::os::windows::io::{AsHandle, AsRawHandle};
        use std::ptr;

        use interprocess::os::windows::security_descriptor::SecurityDescriptor;
        use widestring::U16CString;
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
        use windows_sys::Win32::Security::Authorization::{
            ConvertSidToStringSidW, GetSecurityInfo, SE_KERNEL_OBJECT,
        };
        use windows_sys::Win32::Security::{
            GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
            TOKEN_QUERY, TOKEN_USER, TokenUser,
        };
        use windows_sys::Win32::System::Pipes::PeekNamedPipe;
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
        };

        use super::LocalSocketStream;
        use crate::PlatformError;

        struct Handle(HANDLE);

        impl Drop for Handle {
            fn drop(&mut self) {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }

        struct LocalAllocation(*mut c_void);

        impl Drop for LocalAllocation {
            fn drop(&mut self) {
                unsafe {
                    LocalFree(self.0);
                }
            }
        }

        pub(super) fn current_user_sid() -> Result<String, PlatformError> {
            current_process_user_sid().map_err(|error| {
                PlatformError::new(format!(
                    "could not read the current Windows user SID: {error}"
                ))
            })
        }

        fn current_process_user_sid() -> io::Result<String> {
            let mut token = ptr::null_mut();
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let token = Handle(token);
            token_user_sid(token.0)
        }

        fn current_thread_user_sid() -> io::Result<String> {
            let mut token = ptr::null_mut();
            if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 0, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let token = Handle(token);
            token_user_sid(token.0)
        }

        fn token_user_sid(token: HANDLE) -> io::Result<String> {
            let mut size = 0;
            unsafe {
                GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
            }
            if size == 0 {
                return Err(io::Error::last_os_error());
            }
            // `TOKEN_USER` contains a pointer and must not be read from a
            // byte-aligned `Vec<u8>`. Word storage supplies pointer alignment
            // while still reserving the variable-sized SID bytes Windows
            // writes after the structure.
            let words = (size as usize).div_ceil(size_of::<usize>());
            let mut storage = vec![0_usize; words];
            debug_assert_eq!(storage.as_ptr().align_offset(align_of::<TOKEN_USER>()), 0);
            if unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    storage.as_mut_ptr().cast::<c_void>(),
                    size,
                    &mut size,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let token_user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
            sid_string(token_user.User.Sid)
        }

        fn sid_string(sid: PSID) -> io::Result<String> {
            let mut string_sid = ptr::null_mut();
            if unsafe { ConvertSidToStringSidW(sid, &mut string_sid) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let allocation = LocalAllocation(string_sid.cast());
            let mut length = 0;
            while unsafe { *string_sid.add(length) } != 0 {
                length += 1;
            }
            let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(string_sid, length) })
                .map_err(io::Error::other);
            drop(allocation);
            sid
        }

        pub(super) fn authenticate_server(
            stream: &LocalSocketStream,
            expected_sid: &str,
        ) -> io::Result<()> {
            let LocalSocketStream::NamedPipe(stream) = stream;
            let mut owner = ptr::null_mut();
            let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
            let status = unsafe {
                GetSecurityInfo(
                    stream.as_handle().as_raw_handle(),
                    SE_KERNEL_OBJECT,
                    OWNER_SECURITY_INFORMATION,
                    &mut owner,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut descriptor,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            if descriptor.is_null() || owner.is_null() {
                if !descriptor.is_null() {
                    unsafe {
                        LocalFree(descriptor.cast());
                    }
                }
                return Err(io::Error::other("the named pipe has no owner SID"));
            }
            let descriptor = LocalAllocation(descriptor.cast());
            let owner_sid = sid_string(owner)?;
            drop(descriptor);
            require_sid(&owner_sid, expected_sid, "pipe owner")
        }

        pub(super) fn authenticate_client(
            stream: &LocalSocketStream,
            expected_sid: &str,
        ) -> io::Result<()> {
            let LocalSocketStream::NamedPipe(stream) = stream;
            // The guard binds the token lookup to this connected pipe instance;
            // unlike a peer PID, the impersonation token cannot be redirected
            // by process exit and PID reuse between two system calls.
            let _impersonation = stream.inner().impersonate_client()?;
            let client_sid = current_thread_user_sid()?;
            require_sid(&client_sid, expected_sid, "pipe client")
        }

        pub(super) fn bytes_available(stream: &LocalSocketStream) -> io::Result<usize> {
            let LocalSocketStream::NamedPipe(stream) = stream;
            let mut available = 0;
            if unsafe {
                PeekNamedPipe(
                    stream.as_handle().as_raw_handle(),
                    ptr::null_mut(),
                    0,
                    ptr::null_mut(),
                    &mut available,
                    ptr::null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(available as usize)
        }

        fn require_sid(actual: &str, expected: &str, peer: &str) -> io::Result<()> {
            if actual == expected {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("the single-instance {peer} belongs to another user"),
                ))
            }
        }

        pub(super) fn security_descriptor(user_sid: &str) -> io::Result<SecurityDescriptor> {
            let sddl = U16CString::from_str(format!("O:{user_sid}D:P(A;;GA;;;{user_sid})"))
                .map_err(io::Error::other)?;
            SecurityDescriptor::deserialize(&sddl)
        }
    }
}

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

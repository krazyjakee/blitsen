//! Managed child processes for CLI-driven desktop applications (#383).
//!
//! An executable and an argument array, never a command line: nothing here is
//! parsed by a shell, so an argument containing a space, a quote or a `$`
//! reaches the child exactly as written. A child is started on a worker
//! thread, each of its pipes is read by a thread of its own, and everything
//! those threads learn is queued here for the frame loop to collect — output
//! in byte chunks, in the order it was read, and the exit after the last of
//! it. No thread of this module ever enters JavaScript.
//!
//! The child is the root of a tree the module owns as a whole. On Unix it
//! leads a process group of its own, and terminating it signals the group; on
//! Windows it is assigned to a job object, and terminating it terminates the
//! job. Both are what make "cancel" mean the tools the child started as well
//! as the child. The tree dies with the child on both: a descendant still
//! running when the child exits is killed, as a job kills it when its last
//! handle closes, so the two platforms answer the same way. A descendant that
//! starts a session of its own — a daemon, deliberately — escapes that, as it
//! would escape a job.
//!
//! When this process exits, every tree still registered is killed: on Windows
//! by the job handles closing, on Unix by an `atexit` handler, which runs for
//! `std::process::exit` as well as for a `main` that returns. Linux adds
//! `PR_SET_PDEATHSIG` so the child is killed even if this process is.
//!
//! Absent on Android, which has no process to spawn a developer tool with.

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use parking_lot::Mutex;

use crate::PlatformError;

/// How one of the child's standard streams is connected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StdioMode {
    /// A pipe this module reads or writes.
    Piped,
    /// The stream this process has, which a desktop application may not.
    Inherit,
    /// Nothing: reads see end of file, writes are discarded.
    Null,
}

impl StdioMode {
    fn stdio(self) -> Stdio {
        match self {
            Self::Piped => Stdio::piped(),
            Self::Inherit => Stdio::inherit(),
            Self::Null => Stdio::null(),
        }
    }
}

/// What to start.
#[derive(Clone, Debug)]
pub struct SpawnRequest {
    /// The executable: a path, or a name looked up on `PATH`.
    pub program: String,
    /// Its arguments, each one an `argv` element.
    pub args: Vec<String>,
    /// The working directory, or this process's.
    pub cwd: Option<PathBuf>,
    /// Variables to set, or with `None` to remove, over the inherited set.
    pub env: Vec<(String, Option<String>)>,
    /// Whether the child starts from this process's environment.
    pub inherit_env: bool,
    /// How the child's stdin is connected.
    pub stdin: StdioMode,
    /// How the child's stdout is connected.
    pub stdout: StdioMode,
    /// How the child's stderr is connected.
    pub stderr: StdioMode,
}

/// Why a command did not do what was asked, named for the `DOMException` the
/// bridge raises.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure {
    /// The program, or the working directory, does not exist.
    NotFound(String),
    /// The program exists and this process may not run it.
    NotAllowed(String),
    /// The child is not in a state that accepts the command.
    InvalidState(String),
    /// The platform failed for a reason none of the above describes.
    Operation(String),
}

impl Failure {
    /// The `DOMException` name an application branches on.
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "NotFoundError",
            Self::NotAllowed(_) => "NotAllowedError",
            Self::InvalidState(_) => "InvalidStateError",
            Self::Operation(_) => "OperationError",
        }
    }

    /// The text written for a person reading a log.
    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(message)
            | Self::NotAllowed(message)
            | Self::InvalidState(message)
            | Self::Operation(message) => message,
        }
    }
}

/// Which output stream a chunk came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// A child that started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spawned {
    /// The handle every later command and event names it by.
    pub id: u64,
    /// The operating system's process id.
    pub pid: u32,
}

/// How a child ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExitStatus {
    /// The exit code, or `None` when a signal ended it.
    pub code: Option<i32>,
    /// The signal that ended it, by name, or `None` when it exited.
    pub signal: Option<String>,
}

/// One frame-turn message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// A spawn command settled.
    Spawned {
        /// The command it answers.
        command_id: u64,
        /// The child, or why there is none.
        result: Result<Spawned, Failure>,
    },
    /// A stdin write settled.
    Written {
        /// The command it answers.
        command_id: u64,
        /// Whether the bytes reached the child.
        result: Result<(), Failure>,
    },
    /// A chunk of output, in the order it was read from its stream.
    Output {
        /// The child it came from.
        id: u64,
        /// Which stream.
        stream: Stream,
        /// The bytes, unsplit and undecoded.
        data: Vec<u8>,
    },
    /// The one terminal event a child produces, after the last of its output.
    Exited {
        /// The child that ended.
        id: u64,
        /// How.
        status: ExitStatus,
    },
}

/// A queued stdin write.
struct StdinWrite {
    command_id: u64,
    data: Vec<u8>,
}

/// A live child, as the registry sees it.
struct Entry {
    tree: platform::Tree,
    stdin: Option<mpsc::Sender<StdinWrite>>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
// Reload invalidates launches that have not reached the registry yet.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Every child that has started and not yet ended, by handle.
static PROCESSES: Mutex<BTreeMap<u64, Entry>> = Mutex::new(BTreeMap::new());
/// What the worker threads learned, waiting for a frame turn. Process-visible
/// and `Sync`, the way the dialog and shell queues are, because every message
/// is produced on a thread that is not the frame loop's.
static EVENTS: Mutex<VecDeque<Event>> = Mutex::new(VecDeque::new());

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn push(event: Event) {
    EVENTS.lock().push_back(event);
}

/// Starts a child, returning the id its [`Event::Spawned`] will carry.
///
/// The spawn happens on a worker thread, which then supervises the child for
/// its whole life; a launch failure is a `Spawned` event carrying the reason,
/// not an error here.
pub fn spawn(request: SpawnRequest) -> u64 {
    let command_id = next_id();
    let generation = GENERATION.load(Ordering::Acquire);
    platform::register_exit_cleanup();
    let started = std::thread::Builder::new()
        .name("blitsen-process".to_owned())
        .spawn(move || match launch(&request) {
            Ok(child) => supervise(command_id, generation, child),
            Err(failure) => push(Event::Spawned {
                command_id,
                result: Err(failure),
            }),
        });
    if let Err(error) = started {
        push(Event::Spawned {
            command_id,
            result: Err(Failure::Operation(format!(
                "could not start a supervisor thread: {error}"
            ))),
        });
    }
    command_id
}

/// Queues bytes for the child's stdin, returning the id its
/// [`Event::Written`] will carry.
///
/// Refuses synchronously when there is no such child or its stdin is not a
/// pipe this module holds: both are facts the caller already has.
pub fn write(id: u64, data: Vec<u8>) -> Result<u64, PlatformError> {
    let command_id = next_id();
    let processes = PROCESSES.lock();
    let entry = processes
        .get(&id)
        .ok_or_else(|| PlatformError::new(format!("there is no child process {id}")))?;
    let stdin = entry
        .stdin
        .as_ref()
        .ok_or_else(|| PlatformError::new(format!("child process {id}'s stdin is not open")))?;
    stdin
        .send(StdinWrite { command_id, data })
        .map_err(|_| PlatformError::new(format!("child process {id}'s stdin is closed")))?;
    Ok(command_id)
}

/// Closes the child's stdin once every queued write has been made.
///
/// Closing twice, or closing a child that has ended, is nothing.
pub fn close_stdin(id: u64) {
    if let Some(entry) = PROCESSES.lock().get_mut(&id) {
        entry.stdin = None;
    }
}

/// Terminates the child's whole tree: politely, or with `force`, at once.
///
/// On Unix that is `SIGTERM` or `SIGKILL` to the process group; on Windows,
/// where there is no polite signal, both terminate the job. A child that has
/// already ended is left alone.
pub fn kill(id: u64, force: bool) {
    if let Some(entry) = PROCESSES.lock().get(&id) {
        entry.tree.kill(force);
    }
}

/// Whether any message is waiting for a frame turn.
pub fn pending() -> bool {
    !EVENTS.lock().is_empty()
}

/// Drains the messages queued since the last call.
pub fn take() -> Vec<Event> {
    EVENTS.lock().drain(..).collect()
}

/// Kills every tree and forgets every child, for a document that is going away.
///
/// A reload must not leave the old document's tools running under the new one,
/// and the new one has no handle to reach them by. Forced rather than polite,
/// so it is finished when it returns.
pub fn dispose_all() {
    let processes = {
        let mut processes = PROCESSES.lock();
        GENERATION.fetch_add(1, Ordering::Release);
        std::mem::take(&mut *processes)
    };
    for entry in processes.values() {
        entry.tree.kill(true);
    }
    EVENTS.lock().clear();
}

/// Builds and starts the command, mapping the ways that can fail.
fn launch(request: &SpawnRequest) -> Result<Child, Failure> {
    if let Some(cwd) = &request.cwd
        && !cwd.is_dir()
    {
        return Err(Failure::NotFound(format!(
            "the working directory {} does not exist",
            cwd.display()
        )));
    }
    let mut command = Command::new(&request.program);
    command.args(&request.args);
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    if !request.inherit_env {
        command.env_clear();
    }
    for (name, value) in &request.env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    command
        .stdin(request.stdin.stdio())
        .stdout(request.stdout.stdio())
        .stderr(request.stderr.stdio());
    platform::prepare(&mut command);
    command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => {
            Failure::NotFound(format!("{} was not found", request.program))
        }
        std::io::ErrorKind::PermissionDenied => Failure::NotAllowed(format!(
            "{} may not be executed by this process",
            request.program
        )),
        _ => Failure::Operation(format!("{} could not be started: {error}", request.program)),
    })
}

/// Owns a child for its whole life, from the thread that started it.
fn supervise(command_id: u64, generation: u64, mut child: Child) {
    let id = next_id();
    let pid = child.id();
    let tree = match platform::Tree::adopt(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            push(Event::Spawned {
                command_id,
                result: Err(Failure::Operation(error.to_string())),
            });
            return;
        }
    };
    let mut processes = PROCESSES.lock();
    if generation != GENERATION.load(Ordering::Acquire) {
        tree.kill(true);
        let _ = child.kill();
        let _ = child.wait();
        return;
    }
    if let Err(error) = tree.resume(&child) {
        tree.kill(true);
        let _ = child.kill();
        let _ = child.wait();
        push(Event::Spawned {
            command_id,
            result: Err(Failure::Operation(error.to_string())),
        });
        return;
    }
    let stdin = child.stdin.take().map(|stdin| {
        let (sender, receiver) = mpsc::channel();
        spawn_named("blitsen-process-stdin", move || {
            write_stdin(stdin, receiver)
        });
        sender
    });
    // Registered before the answer is queued, so a write that follows the
    // resolved spawn on the very next line finds the child.
    processes.insert(id, Entry { tree, stdin });
    drop(processes);
    push(Event::Spawned {
        command_id,
        result: Ok(Spawned { id, pid }),
    });
    let readers = [
        (
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
            Stream::Stdout,
        ),
        (
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
            Stream::Stderr,
        ),
    ]
    .into_iter()
    .filter_map(|(pipe, stream)| {
        pipe.map(|pipe| {
            spawn_named("blitsen-process-output", move || {
                read_output(id, stream, pipe)
            })
        })
    })
    .collect::<Vec<_>>();
    let status = child.wait();
    // Descendants may still hold the output pipes open. End the tree before
    // joining readers, otherwise EOF (and the exit event) may never arrive.
    if let Some(entry) = PROCESSES.lock().get(&id) {
        entry.tree.kill(true);
    }
    // After the last of the output: the readers stop at end of file, which is
    // when the child and everything it handed its pipes to have let go.
    for reader in readers.into_iter().flatten() {
        let _ = reader.join();
    }
    let status = match status {
        Ok(status) => platform::exit_status(status),
        Err(error) => {
            // The wait itself failing is the platform losing track of the child.
            ExitStatus {
                code: None,
                signal: Some(format!("lost: {error}")),
            }
        }
    };
    if let Some(entry) = PROCESSES.lock().remove(&id) {
        // The tree dies with its root, on both platforms alike.
        entry.tree.kill(true);
    }
    push(Event::Exited { id, status });
}

fn spawn_named(
    name: &str,
    body: impl FnOnce() + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .ok()
}

/// Reads one pipe to its end, queuing each chunk as it arrives.
fn read_output(id: u64, stream: Stream, mut pipe: Box<dyn Read + Send>) {
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => push(Event::Output {
                id,
                stream,
                data: buffer[..read].to_vec(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// Makes every queued write, then lets the pipe close when the queue does.
fn write_stdin(mut stdin: ChildStdin, receiver: mpsc::Receiver<StdinWrite>) {
    while let Ok(write) = receiver.recv() {
        let result = stdin
            .write_all(&write.data)
            .and_then(|()| stdin.flush())
            .map_err(|error| Failure::Operation(format!("stdin: {error}")));
        push(Event::Written {
            command_id: write.command_id,
            result,
        });
    }
}

#[cfg(unix)]
mod platform {
    use std::process::Command;
    use std::sync::Once;

    use super::ExitStatus;

    /// The process group the child leads.
    pub(super) struct Tree {
        group: libc::pid_t,
    }

    impl Tree {
        pub(super) fn adopt(child: &std::process::Child) -> std::io::Result<Self> {
            Ok(Self {
                group: child.id() as libc::pid_t,
            })
        }

        pub(super) fn resume(&self, _child: &std::process::Child) -> std::io::Result<()> {
            Ok(())
        }

        pub(super) fn kill(&self, force: bool) {
            let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
            // SAFETY: `killpg` takes a group id and a signal number and touches
            // no memory of ours. A group that has already gone is `ESRCH`,
            // which is the answer this wants.
            unsafe { libc::killpg(self.group, signal) };
        }
    }

    /// Puts the child in a process group of its own, and on Linux has the
    /// kernel kill it if this process dies without running its exit handlers.
    pub(super) fn prepare(command: &mut Command) {
        use std::os::unix::process::CommandExt;

        command.process_group(0);
        #[cfg(target_os = "linux")]
        {
            // SAFETY: runs in the forked child before `exec`. `prctl` is
            // async-signal-safe and allocates nothing.
            unsafe {
                command.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                    Ok(())
                });
            }
        }
    }

    pub(super) fn exit_status(status: std::process::ExitStatus) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;

        ExitStatus {
            code: status.code(),
            signal: status.signal().map(signal_name),
        }
    }

    /// The name a shell would print for a signal, on this platform's numbering.
    fn signal_name(signal: i32) -> String {
        let name = match signal {
            libc::SIGHUP => "SIGHUP",
            libc::SIGINT => "SIGINT",
            libc::SIGQUIT => "SIGQUIT",
            libc::SIGILL => "SIGILL",
            libc::SIGTRAP => "SIGTRAP",
            libc::SIGABRT => "SIGABRT",
            libc::SIGBUS => "SIGBUS",
            libc::SIGFPE => "SIGFPE",
            libc::SIGKILL => "SIGKILL",
            libc::SIGUSR1 => "SIGUSR1",
            libc::SIGSEGV => "SIGSEGV",
            libc::SIGUSR2 => "SIGUSR2",
            libc::SIGPIPE => "SIGPIPE",
            libc::SIGALRM => "SIGALRM",
            libc::SIGTERM => "SIGTERM",
            other => return format!("SIG{other}"),
        };
        name.to_owned()
    }

    extern "C" fn kill_every_tree() {
        // Other threads are still running inside `exit`, so a lock that cannot
        // be taken is skipped rather than waited for: the alternative is an
        // exit that never finishes.
        if let Some(processes) = super::PROCESSES.try_lock() {
            for entry in processes.values() {
                entry.tree.kill(true);
            }
        }
    }

    /// Registers the exit handler once, the first time anything is spawned.
    pub(super) fn register_exit_cleanup() {
        static REGISTERED: Once = Once::new();
        REGISTERED.call_once(|| {
            // SAFETY: `kill_every_tree` is an `extern "C"` function with no
            // arguments, which is what `atexit` runs.
            unsafe { libc::atexit(kill_every_tree) };
        });
    }
}

#[cfg(windows)]
mod platform {
    use std::os::windows::io::AsRawHandle;
    use std::process::Command;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    use super::ExitStatus;

    /// The exit code a terminated tree reports. `1`, as `TerminateProcess`
    /// callers conventionally use; the bridge's `killed` flag is what tells a
    /// termination from a child that exited with 1 itself.
    const TERMINATED: u32 = 1;

    /// The job object assigned before the child's first instruction runs.
    pub(super) struct Tree {
        job: HANDLE,
    }

    // SAFETY: a job handle is a kernel object reference that any thread may
    // use; the registry mutex serialises the uses this module makes.
    unsafe impl Send for Tree {}

    impl Tree {
        pub(super) fn adopt(child: &std::process::Child) -> std::io::Result<Self> {
            // SAFETY: null pointers request default security and an unnamed
            // job. Every successful handle is owned by Tree from this point.
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let tree = Self { job };
            // SAFETY: the fully initialized limits structure lives through the
            // call and the child handle belongs to a live, suspended Child.
            unsafe {
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                    || AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(tree)
        }

        pub(super) fn resume(&self, child: &std::process::Child) -> std::io::Result<()> {
            // Stable std does not expose Child's primary thread handle. The
            // suspended process has exactly its initial thread; find that
            // thread through Toolhelp and resume it only after job assignment.
            // SAFETY: the snapshot and opened thread are closed on every path;
            // THREADENTRY32 has the documented size before enumeration starts.
            unsafe {
                let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
                if snapshot == INVALID_HANDLE_VALUE {
                    return Err(std::io::Error::last_os_error());
                }
                let result = (|| {
                    let mut entry: THREADENTRY32 = std::mem::zeroed();
                    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
                    let mut found = Thread32First(snapshot, &raw mut entry);
                    while found != 0 {
                        if entry.th32OwnerProcessID == child.id() {
                            let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                            if thread.is_null() {
                                return Err(std::io::Error::last_os_error());
                            }
                            let resumed = ResumeThread(thread);
                            let error = std::io::Error::last_os_error();
                            CloseHandle(thread);
                            return if resumed == u32::MAX {
                                Err(error)
                            } else {
                                Ok(())
                            };
                        }
                        found = Thread32Next(snapshot, &raw mut entry);
                    }
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "the suspended child's initial thread was not found",
                    ))
                })();
                CloseHandle(snapshot);
                result
            }
        }

        pub(super) fn kill(&self, _force: bool) {
            // SAFETY: job is the valid handle owned by this Tree.
            unsafe { TerminateJobObject(self.job, TERMINATED) };
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            if !self.job.is_null() {
                // SAFETY: closes the handle `adopt` created, once. Kill-on-close
                // ends whatever is still in the job.
                unsafe { CloseHandle(self.job) };
            }
        }
    }

    /// A console tool started by a windowed application gets no console window
    /// of its own flashing up.
    pub(super) fn prepare(command: &mut Command) {
        use std::os::windows::process::CommandExt;

        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }

    pub(super) fn exit_status(status: std::process::ExitStatus) -> ExitStatus {
        ExitStatus {
            code: status.code(),
            signal: None,
        }
    }

    /// Nothing to register: the job handles close when this process ends,
    /// and kill-on-close does the rest.
    pub(super) fn register_exit_cleanup() {}
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// The queue is process-wide, so tests that use it take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn request(program: &str, args: &[&str]) -> SpawnRequest {
        SpawnRequest {
            program: program.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: None,
            env: Vec::new(),
            inherit_env: true,
            stdin: StdioMode::Null,
            stdout: StdioMode::Piped,
            stderr: StdioMode::Piped,
        }
    }

    /// Collects events until `done` says the story is complete.
    fn collect(done: impl Fn(&[Event]) -> bool) -> Vec<Event> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut events = Vec::new();
        while !done(&events) {
            assert!(Instant::now() < deadline, "timed out with {events:?}");
            events.extend(take());
            std::thread::sleep(Duration::from_millis(5));
        }
        events
    }

    fn spawned(events: &[Event]) -> Option<&Result<Spawned, Failure>> {
        events.iter().find_map(|event| match event {
            Event::Spawned { result, .. } => Some(result),
            _ => None,
        })
    }

    fn exited(events: &[Event]) -> Option<&ExitStatus> {
        events.iter().find_map(|event| match event {
            Event::Exited { status, .. } => Some(status),
            _ => None,
        })
    }

    fn output(events: &[Event], wanted: Stream) -> Vec<u8> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Output { stream, data, .. } if *stream == wanted => Some(data.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 checks for existence and delivers nothing.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[test]
    fn output_arrives_in_order_and_before_the_exit() {
        let _serial = SERIAL.lock();
        let command_id = spawn(request(
            "sh",
            &["-c", "printf one; printf two; printf err >&2; exit 3"],
        ));
        let events = collect(|events| exited(events).is_some());
        let first = events.first().expect("something happened");
        assert!(
            matches!(first, Event::Spawned { command_id: id, result: Ok(_) } if *id == command_id)
        );
        assert_eq!(output(&events, Stream::Stdout), b"onetwo");
        assert_eq!(output(&events, Stream::Stderr), b"err");
        assert!(matches!(events.last(), Some(Event::Exited { .. })));
        assert_eq!(
            exited(&events),
            Some(&ExitStatus {
                code: Some(3),
                signal: None
            })
        );
        assert!(!pending());
    }

    #[test]
    fn arguments_reach_the_child_unchanged() {
        let _serial = SERIAL.lock();
        let hostile = [
            "plain",
            "with space",
            "\"quoted\"",
            "it's",
            "$HOME `id` ; rm -rf / | cat && echo",
            "ünïcödé → ✓",
            "--flag=value",
            "",
        ];
        let mut args = vec!["-c", r#"for a in "$@"; do printf '%s\n' "$a"; done"#, "--"];
        args.extend(hostile);
        spawn(request("sh", &args));
        let events = collect(|events| exited(events).is_some());
        let stdout = String::from_utf8(output(&events, Stream::Stdout)).expect("utf-8");
        assert_eq!(
            stdout.split('\n').collect::<Vec<_>>()[..hostile.len()],
            hostile
        );
    }

    #[test]
    fn environment_and_working_directory_are_the_callers() {
        let _serial = SERIAL.lock();
        let directory = std::env::temp_dir();
        let mut request = request(
            "sh",
            &[
                "-c",
                "printf '%s|%s|%s' \"$BLITSEN_SET\" \"${HOME-unset}\" \"$PWD\"",
            ],
        );
        request.cwd = Some(directory.clone());
        request.env = vec![
            ("BLITSEN_SET".to_owned(), Some("yes".to_owned())),
            ("HOME".to_owned(), None),
        ];
        spawn(request);
        let events = collect(|events| exited(events).is_some());
        let stdout = String::from_utf8(output(&events, Stream::Stdout)).expect("utf-8");
        let [set, home, pwd] = stdout.split('|').collect::<Vec<_>>()[..] else {
            panic!("unexpected output {stdout:?}");
        };
        assert_eq!(set, "yes");
        assert_eq!(home, "unset");
        assert_eq!(
            std::fs::canonicalize(pwd).ok(),
            std::fs::canonicalize(&directory).ok()
        );
    }

    #[test]
    fn stdin_is_written_and_closed_on_request() {
        let _serial = SERIAL.lock();
        let mut request = request("cat", &[]);
        request.stdin = StdioMode::Piped;
        spawn(request);
        let events = collect(|events| spawned(events).is_some());
        let id = spawned(&events)
            .expect("spawned")
            .clone()
            .expect("started")
            .id;
        let first = write(id, b"hello ".to_vec()).expect("stdin is open");
        let second = write(id, b"world".to_vec()).expect("stdin is open");
        close_stdin(id);
        close_stdin(id);
        let events = collect(|events| exited(events).is_some());
        assert_eq!(output(&events, Stream::Stdout), b"hello world");
        let written = events
            .iter()
            .filter_map(|event| match event {
                Event::Written { command_id, result } => Some((*command_id, result.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(written, vec![(first, Ok(())), (second, Ok(()))]);
        assert_eq!(exited(&events).and_then(|status| status.code), Some(0));
        assert!(
            write(id, b"late".to_vec()).is_err(),
            "a finished child has no stdin"
        );
    }

    #[test]
    fn a_missing_program_is_a_not_found_spawn() {
        let _serial = SERIAL.lock();
        spawn(request("blitsen-no-such-program-383", &[]));
        let events = collect(|events| spawned(events).is_some());
        let failure = spawned(&events)
            .expect("answered")
            .clone()
            .expect_err("not started");
        assert_eq!(failure.name(), "NotFoundError");
        assert!(failure.message().contains("blitsen-no-such-program-383"));
        let mut request = request("sh", &["-c", "true"]);
        request.cwd = Some(PathBuf::from("/nonexistent/blitsen-383"));
        spawn(request);
        let events = collect(|events| spawned(events).is_some());
        let failure = spawned(&events)
            .expect("answered")
            .clone()
            .expect_err("not started");
        assert_eq!(failure.name(), "NotFoundError");
        assert!(failure.message().contains("working directory"));
    }

    #[test]
    fn killing_the_child_ends_its_descendants() {
        let _serial = SERIAL.lock();
        // A child that starts a grandchild, reports its pid, and waits for it.
        spawn(request("sh", &["-c", "sleep 60 & echo $!; wait"]));
        let events = collect(|events| !output(events, Stream::Stdout).is_empty());
        let id = spawned(&events)
            .expect("spawned")
            .clone()
            .expect("started")
            .id;
        let grandchild = String::from_utf8(output(&events, Stream::Stdout))
            .expect("utf-8")
            .trim()
            .parse::<i32>()
            .expect("a pid");
        assert!(alive(grandchild));
        kill(id, false);
        let events = collect(|events| exited(events).is_some());
        assert_eq!(
            exited(&events),
            Some(&ExitStatus {
                code: None,
                signal: Some("SIGTERM".to_owned())
            })
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(grandchild) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!alive(grandchild), "the grandchild outlived the tree");
        kill(id, true);
    }

    #[test]
    fn root_exit_kills_descendants_before_waiting_for_pipe_eof() {
        let _serial = SERIAL.lock();
        spawn(request("sh", &["-c", "sleep 60 & printf done; exit 4"]));
        let events = collect(|events| exited(events).is_some());
        assert_eq!(output(&events, Stream::Stdout), b"done");
        assert_eq!(exited(&events).and_then(|status| status.code), Some(4));
    }

    #[test]
    fn a_launch_in_flight_cannot_register_after_dispose() {
        let _serial = SERIAL.lock();
        let generation = GENERATION.load(Ordering::Acquire);
        let child = launch(&request("sleep", &["60"])).expect("started");
        let pid = child.id();
        dispose_all();
        supervise(next_id(), generation, child);
        assert!(!alive(pid as i32), "the stale launch was killed and reaped");
        assert!(PROCESSES.lock().is_empty());
        assert!(take().is_empty(), "the old document receives no spawn");
    }

    #[test]
    fn disposing_kills_everything_at_once() {
        let _serial = SERIAL.lock();
        spawn(request("sleep", &["60"]));
        let events = collect(|events| spawned(events).is_some());
        let id = spawned(&events)
            .expect("spawned")
            .clone()
            .expect("started")
            .id;
        dispose_all();
        assert!(
            write(id, Vec::new()).is_err(),
            "a disposed child is forgotten"
        );
        let events = collect(|events| exited(events).is_some());
        assert_eq!(
            exited(&events).and_then(|status| status.signal.clone()),
            Some("SIGKILL".to_owned())
        );
    }
}

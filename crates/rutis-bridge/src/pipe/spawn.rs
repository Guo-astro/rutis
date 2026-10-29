//! Starting processes on a channel: the process gets one end of a socket
//! pair as fd 3 (`fd:3`), or, when it cannot take one, a socket path in a
//! private directory that it dials back (Unix); or a loopback TCP address
//! to dial, presenting a one-time token first (every platform; the only
//! one on Windows). The channel owns the process: closing it, or dropping
//! all of it, ends the process, and its end says how the process ended.
use std::ffi::OsString;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::channel::{Channel, ChannelError, Close, Closer, ConnectError, Receiver, Sender};
use std::collections::BTreeSet;
use tokio::sync::{oneshot, watch};

use crate::pipe::lines;

/// The fd a process finds its channel on (`fd:3`).
pub const CHANNEL_FD: i32 = 3;

/// The environment variable holding the token a process started with
/// [`Handover::Loopback`] presents, as its first line, on the address it is
/// given (`tcp:127.0.0.1:<port>`). It should remove it from its own
/// environment once read.
pub const CHANNEL_TOKEN: &str = "RUTIS_CHANNEL_TOKEN";

/// Where a process's standard input, output or error goes. The process
/// that starts a runtime owns it: a stream is this process's own, nothing,
/// or (output only) the write end of a pipe the caller reads.
#[derive(Clone)]
#[non_exhaustive]
pub enum Stdio {
    /// None: no input to read, output discarded (the null device).
    Null,
    /// This process's own stream: its terminal, when it runs in one. As
    /// standard input on Unix, it keeps the process in this process's
    /// group, so a Ctrl-C in the terminal reaches it as well (otherwise it
    /// gets a group of its own, out of the terminal's reach).
    Inherit,
    /// Output only: the stream [`Pipe`] makes for each process started (the
    /// write end of a pipe whose reader passes the output on: rutis-host's,
    /// a whole line at a time). The process does not see a terminal. As
    /// standard input, it is [`Stdio::Null`].
    Pipe(Pipe),
}

/// What makes the stream of a process's output for [`Stdio::Pipe`], once
/// per process started.
pub type Pipe = std::sync::Arc<dyn Fn() -> std::io::Result<std::process::Stdio> + Send + Sync>;

impl std::fmt::Debug for Stdio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Stdio::Null => "Null",
            Stdio::Inherit => "Inherit",
            Stdio::Pipe(_) => "Pipe",
        })
    }
}

impl Stdio {
    /// The stream to give the process: its input when `output` is false.
    pub(crate) fn process(&self, output: bool) -> std::io::Result<std::process::Stdio> {
        Ok(match self {
            Stdio::Null => std::process::Stdio::null(),
            Stdio::Pipe(_) if !output => std::process::Stdio::null(),
            Stdio::Inherit => std::process::Stdio::inherit(),
            Stdio::Pipe(pipe) => pipe()?,
        })
    }
}

/// A process `spawn:<name>` starts: `program args… <channel> trailing…`,
/// where `<channel>` is `fd:3` or the socket path to dial.
///
/// Build one with [`Spawn::new`] and set its fields.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Spawn {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    /// The working directory; the application's by default.
    pub cwd: Option<PathBuf>,
    pub handover: Handover,
    /// Standard input: [`Stdio::Null`] by default.
    pub stdin: Stdio,
    /// Standard output: [`Stdio::Inherit`] by default.
    pub stdout: Stdio,
    /// Standard error: [`Stdio::Inherit`] by default.
    pub stderr: Stdio,
    /// Arguments after the channel.
    pub trailing: Vec<OsString>,
}

/// How the process gets its channel. On Windows, every process gets
/// [`Handover::Loopback`]; elsewhere too when [`HANDOVER_VARIABLE`] is
/// `loopback` (to run Windows' path on Unix).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Handover {
    /// One end of a socket pair, as fd 3.
    Inherit,
    /// A socket path the process dials.
    DialBack,
    /// A loopback TCP address the process dials (`tcp:127.0.0.1:<port>`),
    /// sending the token in [`CHANNEL_TOKEN`] and a newline first; other
    /// connections are turned away.
    Loopback,
}

impl Spawn {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            handover: Handover::Inherit,
            stdin: Stdio::Null,
            stdout: Stdio::Inherit,
            stderr: Stdio::Inherit,
            trailing: Vec::new(),
        }
    }

    fn command(&self) -> Result<tokio::process::Command, ConnectError> {
        let cannot = |error| cannot_start(self, error);
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&self.args)
            .envs(self.env.iter().map(|(name, value)| (name, value)))
            .stdin(self.stdin.process(false).map_err(cannot)?)
            .stdout(self.stdout.process(true).map_err(cannot)?)
            .stderr(self.stderr.process(true).map_err(cannot)?)
            .kill_on_drop(true);
        // Out of the terminal's reach: Ctrl-C there signals this process,
        // which unloads the process's plugins over the channel, and not the
        // process itself, which would end before its cleanups ran. Not when
        // it reads the terminal: a background group reading it is stopped
        // (SIGTTIN), so it stays in this one, and Ctrl-C reaches it too.
        #[cfg(unix)]
        if !matches!(self.stdin, Stdio::Inherit) {
            command.process_group(0);
        }
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        Ok(command)
    }
}

fn retryable(error: std::io::Error) -> ConnectError {
    ConnectError::Retryable {
        reason: error.to_string(),
    }
}

/// Set to `loopback` to start every process with [`Handover::Loopback`].
pub const HANDOVER_VARIABLE: &str = "RUTIS_LOCAL_HANDOVER";

/// Start `spawn` and connect it.
pub(crate) async fn start(spawn: &Spawn, processes: &Processes) -> Result<Channel, ConnectError> {
    let forced =
        !cfg!(unix) || std::env::var_os(HANDOVER_VARIABLE).is_some_and(|value| value == "loopback");
    let handover = match forced {
        true => Handover::Loopback,
        false => spawn.handover,
    };
    let (channel, child, directory) = match handover {
        #[cfg(unix)]
        Handover::Inherit => inherit(spawn)?,
        #[cfg(unix)]
        Handover::DialBack => dial_back(spawn).await?,
        _ => loopback(spawn).await?,
    };
    let Channel {
        sender,
        receiver,
        closer,
    } = channel;
    processes.add(child.ended.clone());
    Ok(Channel {
        sender: Box::new(EndedSender {
            sender,
            ended: child.ended.clone(),
        }),
        receiver: Box::new(Ended {
            receiver,
            ended: Some(child.ended.clone()),
        }),
        closer: Arc::new(Owning {
            closer,
            child,
            _directory: directory,
        }),
    })
}

type Started = (Channel, Child, Option<tempfile::TempDir>);

/// Let only fds 0–2 and, when given, `channel` (as fd 3) cross the exec
/// that starts `command`.
///
/// Every descriptor here is created close-on-exec, but not always
/// atomically: on macOS (no `SOCK_CLOEXEC`, no `pipe2`) std creates a
/// socket pair or a pipe, then marks it. A fork on another thread in
/// between copies it unmarked into that child, and the program the child
/// runs holds it for its whole life (#184): another runtime's channel,
/// which then does not end when its own process does, or the pipe on which
/// std waits for another child's exec, which then waits for this child to
/// end. Which thread's descriptor, and which fork, is a matter of timing,
/// and some are created by std or other libraries rather than rutis; so the
/// child itself marks every descriptor above the ones it keeps before exec,
/// whoever created it. A lock around rutis' own creations and forks would
/// cover neither those nor forks by other code, and would serialize starts.
///
/// macOS: libnotify sets up its globals the first time anything uses them,
/// behind an `os_once`, and its fork handler in the child uses them too. A
/// fork while another thread is inside that first setup leaves the child
/// with a half-taken once, and the child aborts in the fork handler ("BUG IN
/// CLIENT OF LIBPLATFORM: os_once_t is corrupt") before it can exec: the
/// runtime process "exits with signal 9" (#287). Setting the globals up here,
/// once, before this process's first fork, closes that window.
#[cfg(target_os = "macos")]
fn notify_ready_before_fork() {
    extern "C" {
        fn notify_is_valid_token(token: std::ffi::c_int) -> bool;
    }
    static READY: std::sync::Once = std::sync::Once::new();
    // SAFETY: a read-only query on a token nobody holds (a negative one could
    // be refused before the globals are touched); it sets up the globals.
    READY.call_once(|| unsafe {
        notify_is_valid_token(std::ffi::c_int::MAX);
    });
}

/// Running a `pre_exec` hook makes std fork rather than `posix_spawn`.
#[cfg(unix)]
pub(crate) fn hand_over(
    command: &mut tokio::process::Command,
    channel: Option<std::os::fd::RawFd>,
) {
    #[cfg(target_os = "macos")]
    notify_ready_before_fork();
    let first = match channel {
        Some(_) => CHANNEL_FD + 1,
        None => CHANNEL_FD,
    };
    // Computed here: sysconf need not be async-signal-safe.
    let limit = descriptor_limit();
    // SAFETY: between fork and exec, only async-signal-safe calls: dup2,
    // fcntl and close_range; nothing allocates.
    unsafe {
        command.pre_exec(move || {
            match channel {
                Some(fd) if fd == CHANNEL_FD => {
                    // dup2 onto itself would leave it close-on-exec.
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Some(fd) if libc::dup2(fd, CHANNEL_FD) < 0 => {
                    return Err(std::io::Error::last_os_error());
                }
                Some(_) | None => {}
            }
            close_on_exec_from(first, limit)
        });
    }
}

/// One past the highest descriptor number to check: the process's limit,
/// at most 65536. Without `close_range` the child checks every number
/// below it, and numbers above that are not handed out in practice.
#[cfg(unix)]
fn descriptor_limit() -> i32 {
    // SAFETY: sysconf has no preconditions.
    let limit = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    match limit {
        ..=0 => 1 << 16, // unknown or unlimited
        limit => limit.min(1 << 16) as i32,
    }
}

/// Mark every descriptor from `first` on close-on-exec, leaving those that
/// already are (std's own exec-error pipe among them) as they are. Runs
/// between fork and exec.
#[cfg(unix)]
fn close_on_exec_from(first: i32, limit: i32) -> std::io::Result<()> {
    // Linux 5.11+ does it in one call.
    #[cfg(target_os = "linux")]
    {
        // SAFETY: a plain syscall with integer arguments.
        let marked = unsafe {
            libc::syscall(
                libc::SYS_close_range,
                first as libc::c_uint,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            )
        };
        if marked == 0 {
            return Ok(());
        }
    }
    for fd in first..limit {
        // SAFETY: fcntl on a number that may not be open: EBADF, skipped.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0
                && flags & libc::FD_CLOEXEC == 0
                && libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

/// Isolate a command that only reads something, such as a binary's
/// `--rutis-manifest`: it runs in a process group of its own, so what it
/// starts can be ended with it, and every descriptor above 2 is
/// close-on-exec, so it holds none of this process's channels or files.
pub fn isolate_manifest_command(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        #[cfg(target_os = "macos")]
        notify_ready_before_fork();
        command.process_group(0);
        // Computed here: sysconf need not be async-signal-safe.
        let limit = descriptor_limit();
        // SAFETY: between fork and exec, only async-signal-safe calls:
        // fcntl and close_range; nothing allocates.
        unsafe {
            command.pre_exec(move || close_on_exec_from(CHANNEL_FD, limit));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = command;
}

/// The manifest readers not done yet: [`kill_processes`] kills them too.
static MANIFESTS: std::sync::LazyLock<Mutex<Vec<Arc<ManifestChildInner>>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

struct ManifestChildInner {
    pid: u32,
    /// Cleared once it was killed, or once its pid may name something else.
    armed: AtomicBool,
    /// A job that holds the process and what it started; 0 when the
    /// process could not be put in one. Windows has no process group to
    /// kill a whole tree through.
    #[cfg(windows)]
    job: usize,
}

/// A process started to print a manifest, leading its own group
/// ([`isolate_manifest_command`]): dropping it before [`disarm`] kills that
/// group, and [`kill_processes`] kills it as well.
///
/// [`disarm`]: ManifestChild::disarm
pub struct ManifestChild {
    inner: Option<Arc<ManifestChildInner>>,
}

impl ManifestChild {
    /// Guard the process `pid`, which must lead its own process group
    /// (on Windows, put it in a job of its own).
    pub fn new(pid: u32) -> Self {
        #[cfg(windows)]
        let job = manifest_job(pid);
        let inner = Arc::new(ManifestChildInner {
            pid,
            armed: AtomicBool::new(true),
            #[cfg(windows)]
            job,
        });
        MANIFESTS.lock().unwrap().push(inner.clone());
        Self { inner: Some(inner) }
    }

    /// Kill its process group now, and whatever it started with it.
    pub fn kill(&self) {
        if let Some(inner) = &self.inner {
            kill_inner(inner);
        }
    }

    /// Stop guarding it: it was waited for, so its pid (and group id) may
    /// name another process now.
    pub fn disarm(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        inner.armed.store(false, Ordering::SeqCst);
        #[cfg(windows)]
        disarm_job(inner.job);
        MANIFESTS
            .lock()
            .unwrap()
            .retain(|registered| !Arc::ptr_eq(registered, &inner));
    }
}

impl Drop for ManifestChild {
    fn drop(&mut self) {
        self.kill();
        self.disarm();
    }
}

/// Kill what the guarded process started, if it is still armed.
fn kill_inner(inner: &ManifestChildInner) -> bool {
    if !inner.armed.swap(false, Ordering::SeqCst) {
        return false;
    }
    #[cfg(windows)]
    if inner.job != 0 {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: the job handle lives until `disarm`, in this guard.
        unsafe {
            TerminateJobObject(inner.job as _, 1);
        }
        return true;
    }
    kill_pid_group(inner.pid);
    true
}

/// A job holding the process `pid`, killed when it closes, or 0 if the
/// process cannot be put in one (then killing falls back to its pid).
#[cfg(windows)]
fn manifest_job(pid: u32) -> usize {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };
    // SAFETY: plain Win32 calls on handles this function owns.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return 0;
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let sized = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            sized,
        ) == 0
        {
            CloseHandle(job);
            return 0;
        }
        let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            CloseHandle(job);
            return 0;
        }
        let assigned = AssignProcessToJobObject(job, process);
        CloseHandle(process);
        match assigned {
            0 => {
                CloseHandle(job);
                0
            }
            _ => job as usize,
        }
    }
}

/// Let what the manifest started go: the manifest was used, so it is the
/// binary's own business. Clearing the limit first keeps closing the job
/// from killing it.
#[cfg(windows)]
fn disarm_job(job: usize) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    };
    if job == 0 {
        return;
    }
    // SAFETY: the job handle is this guard's, and is closed once.
    unsafe {
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = 0;
        let sized = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
        SetInformationJobObject(
            job as _,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            sized,
        );
        CloseHandle(job as _);
    }
}

/// Kill the process group `pid` leads (the process itself on Windows).
fn kill_pid_group(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: kill(2) on the group of a process this process started
        // and has not waited for: neither its id nor its group's can have
        // been reused.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, PROCESS_TERMINATE,
        };
        // SAFETY: plain Win32 calls on a handle this function owns.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !handle.is_null() {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    let _ = pid;
}

/// Kill every manifest reader not done yet, and whatever it started.
fn kill_manifests() {
    let manifests: Vec<Arc<ManifestChildInner>> = MANIFESTS.lock().unwrap().clone();
    for manifest in manifests {
        kill_inner(&manifest);
    }
}

#[cfg(unix)]
fn inherit(spawn: &Spawn) -> Result<Started, ConnectError> {
    let (ours, theirs) = UnixStream::pair().map_err(retryable)?;
    let mut command = spawn.command()?;
    hand_over(&mut command, Some(theirs.as_raw_fd()));
    let child = command
        .arg(format!("fd:{CHANNEL_FD}"))
        .args(&spawn.trailing)
        .spawn()
        .map_err(|error| cannot_start(spawn, error))?;
    // The child has its copy; ours would keep the channel open after it exits.
    drop(theirs);
    Ok((socket(ours)?, Child::watch(child), None))
}

#[cfg(unix)]
async fn dial_back(spawn: &Spawn) -> Result<Started, ConnectError> {
    let directory = tempfile::Builder::new()
        .prefix("rutis-spawn-")
        .tempdir()
        .map_err(retryable)?;
    let path = directory.path().join("peer.sock");
    let listener = tokio::net::UnixListener::bind(&path).map_err(retryable)?;
    let mut command = spawn.command()?;
    hand_over(&mut command, None);
    let mut child = command
        .arg(&path)
        .args(&spawn.trailing)
        .spawn()
        .map_err(|error| cannot_start(spawn, error))?;
    let stream = tokio::select! {
        accepted = listener.accept() => accepted.map_err(retryable)?.0,
        status = child.wait() => return Err(ConnectError::Retryable {
            reason: format!("the process exited before connecting: {}", describe(status)),
        }),
    };
    let stream = stream.into_std().map_err(retryable)?;
    Ok((socket(stream)?, Child::watch(child), Some(directory)))
}

/// How long a connection to the loopback listener may take to present its
/// token.
pub(crate) const TOKEN_WAIT: Duration = Duration::from_secs(10);

async fn loopback(spawn: &Spawn) -> Result<Started, ConnectError> {
    let listener = Loopback::bind().await.map_err(retryable)?;
    let mut command = spawn.command()?;
    #[cfg(unix)]
    hand_over(&mut command, None);
    let mut child = command
        .env(CHANNEL_TOKEN, listener.token())
        .arg(listener.address())
        .args(&spawn.trailing)
        .spawn()
        .map_err(|error| cannot_start(spawn, error))?;
    // A process that would outlive its host is not started.
    if let Err(reason) = adopt(&child) {
        let _ = child.start_kill();
        return Err(ConnectError::Incompatible {
            reason: format!(
                "cannot tie {} to this process's lifetime: {reason}",
                spawn.program.to_string_lossy()
            ),
        });
    }
    let channel = tokio::select! {
        channel = listener.accept() => channel.map_err(retryable)?,
        status = child.wait() => return Err(ConnectError::Retryable {
            reason: format!("the process exited before connecting: {}", describe(status)),
        }),
    };
    Ok((channel, Child::watch(child), None))
}

/// A loopback listener for one process, and the token that process
/// presents ([`Handover::Loopback`]).
pub(crate) struct Loopback {
    listener: tokio::net::TcpListener,
    address: std::net::SocketAddr,
    token: String,
    /// How long a connection may take to present the token.
    pub(crate) wait: Duration,
}

impl Loopback {
    pub(crate) async fn bind() -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        Ok(Self {
            listener,
            address,
            token: token()?,
            wait: TOKEN_WAIT,
        })
    }

    /// The channel argument the process gets: `tcp:<host>:<port>`.
    pub(crate) fn address(&self) -> String {
        format!("tcp:{}", self.address)
    }

    /// The token, for the process's [`CHANNEL_TOKEN`].
    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    /// The channel of the first connection that presents the token. Anyone
    /// on this machine can connect: only the process knows the token. Each
    /// connection presents it on its own, so one that stays silent does not
    /// hold up the process's; those still presenting when it has are
    /// dropped. Waits for ever: the caller races it with the process.
    pub(crate) async fn accept(self) -> std::io::Result<Channel> {
        let mut presenting = tokio::task::JoinSet::new();
        let stream = loop {
            tokio::select! {
                accepted = self.listener.accept() => {
                    let (mut stream, _) = accepted?;
                    let token = self.token.clone();
                    let wait = self.wait;
                    presenting.spawn(async move {
                        let presented = tokio::time::timeout(wait, read_line(&mut stream)).await;
                        matches!(presented, Ok(Ok(line)) if line == token).then_some(stream)
                    });
                }
                Some(presented) = presenting.join_next(), if !presenting.is_empty() => {
                    if let Ok(Some(stream)) = presented {
                        break stream;
                    }
                }
            }
        };
        drop(presenting);
        tcp(stream.into_std()?)
    }
}

/// A loopback connection as a channel, one message per line.
fn tcp(stream: std::net::TcpStream) -> std::io::Result<Channel> {
    stream.set_nonblocking(false)?;
    let _ = stream.set_nodelay(true);
    let reader = stream.try_clone()?;
    let closer = Arc::new(ShutTcp(Mutex::new(Some(stream.try_clone()?))));
    Ok(lines::channel(reader, stream, closer))
}

/// The started process's end of a handover: the channel argument it was
/// given, `fd:<n>` (an inherited socket, Unix) or `tcp:<host>:<port>` (a
/// loopback address, the token in [`CHANNEL_TOKEN`] presented first and
/// then removed from this process's environment).
pub(crate) async fn given(channel: &str) -> Result<Channel, ConnectError> {
    let incompatible = |reason: String| ConnectError::Incompatible { reason };
    if let Some(fd) = channel.strip_prefix("fd:") {
        let fd: i32 = fd
            .parse()
            .map_err(|_| incompatible(format!("{channel}: not a descriptor")))?;
        return given_fd(channel, fd);
    }
    let Some(address) = channel.strip_prefix("tcp:") else {
        return Err(incompatible(format!(
            "{channel}: a channel is fd:<n> or tcp:<host>:<port>"
        )));
    };
    let token = std::env::var(CHANNEL_TOKEN)
        .map_err(|_| incompatible(format!("{channel}: {CHANNEL_TOKEN} is not set")))?;
    std::env::remove_var(CHANNEL_TOKEN);
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .map_err(|error| retryable_at(channel, error))?;
    use tokio::io::AsyncWriteExt;
    stream
        .write_all(format!("{token}\n").as_bytes())
        .await
        .map_err(|error| retryable_at(channel, error))?;
    let stream = stream
        .into_std()
        .map_err(|error| retryable_at(channel, error))?;
    tcp(stream).map_err(|error| retryable_at(channel, error))
}

#[cfg(unix)]
fn given_fd(channel: &str, fd: i32) -> Result<Channel, ConnectError> {
    use std::os::fd::FromRawFd;
    // SAFETY: the descriptor was handed to this process for its channel;
    // nothing else in it uses that number. fstat checks it is open.
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if fd < 0 || unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return Err(ConnectError::Incompatible {
            reason: format!("{channel}: no such descriptor"),
        });
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    // Not handed on to processes this one starts.
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    socket(stream)
}

#[cfg(not(unix))]
fn given_fd(channel: &str, _fd: i32) -> Result<Channel, ConnectError> {
    Err(ConnectError::Incompatible {
        reason: format!("{channel}: this platform hands no descriptor over; use tcp:"),
    })
}

fn retryable_at(channel: &str, error: std::io::Error) -> ConnectError {
    ConnectError::Retryable {
        reason: format!("{channel}: {error}"),
    }
}

/// A random token, hex encoded.
fn token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The first line of `stream`, without its newline; at most 256 bytes.
async fn read_line(stream: &mut tokio::net::TcpStream) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;
    let mut line = Vec::new();
    loop {
        let byte = stream.read_u8().await?;
        if byte == b'\n' {
            return Ok(String::from_utf8_lossy(&line).into_owned());
        }
        if line.len() == 256 {
            return Err(std::io::Error::other("token line too long"));
        }
        line.push(byte);
    }
}

/// Shuts a loopback connection down, then lets go of its handle of it, as
/// `unix::Shut` does.
struct ShutTcp(Mutex<Option<std::net::TcpStream>>);

impl Closer for ShutTcp {
    fn close(&self, _why: Close) {
        let stream = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(stream) = stream {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// A program that cannot be started is configuration, not a passing fault.
fn cannot_start(spawn: &Spawn, error: std::io::Error) -> ConnectError {
    let reason = format!("cannot start {}: {error}", spawn.program.to_string_lossy());
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
            ConnectError::Incompatible { reason }
        }
        _ => ConnectError::Retryable { reason },
    }
}

#[cfg(unix)]
fn socket(stream: UnixStream) -> Result<Channel, ConnectError> {
    stream.set_nonblocking(false).map_err(retryable)?;
    let reader = stream.try_clone().map_err(retryable)?;
    let closer = Arc::new(crate::pipe::unix::Shut::new(
        stream.try_clone().map_err(retryable)?,
    ));
    Ok(lines::channel(reader, stream, closer))
}

/// A closer that owns its process: dropped, it ends the process.
struct Owning {
    closer: Arc<dyn Closer>,
    child: Child,
    _directory: Option<tempfile::TempDir>,
}

impl Closer for Owning {
    fn close(&self, why: Close) {
        self.child.ended.closed_here();
        self.closer.close(why);
        self.child.end();
    }
}

/// Ends the channel, however it ends, with how the process ended.
struct Ended {
    receiver: Box<dyn Receiver>,
    ended: Option<Arc<Exit>>,
}

impl Receiver for Ended {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
        match self.receiver.recv() {
            Ok(Some(message)) => Ok(Some(message)),
            _ => Err(ChannelError::Closed {
                reason: match self.ended.take() {
                    Some(ended) => match ended.wait(HANG_GUARD) {
                        Some(status) => format!("the process {status}"),
                        None => format!(
                            "the process closed its channel but has not exited after {}s",
                            HANG_GUARD.as_secs_f32()
                        ),
                    },
                    None => "the process disconnected".into(),
                },
            }),
        }
    }
}

/// A process that ends before reading (exiting at once, say) fails the
/// first send rather than the first receive: that error says how it ended
/// too, instead of `Broken pipe`.
struct EndedSender {
    sender: Box<dyn Sender>,
    ended: Arc<Exit>,
}

impl Sender for EndedSender {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        self.sender.send(message).map_err(|error| {
            // Refused by the framing itself: nothing to do with the process.
            if lines::refuses(message) {
                return error;
            }
            // Closed here, before or while waiting: the session ends for
            // that, and does not wait for the process to be killed.
            match self.ended.wait_unless_closed(HANG_GUARD) {
                Some(status) => ChannelError::Closed {
                    reason: format!("the process {status}"),
                },
                None => error,
            }
        })
    }

    fn max_message(&self) -> Option<usize> {
        self.sender.max_message()
    }
}

/// How the process ended, once it has. Readable from the reader thread,
/// which must not depend on the async runtime: a current-thread runtime may
/// be blocked in a synchronous call. The fourth field: the process has been
/// waited for, and is gone. The fifth: notified to kill it now. The last:
/// its process id, when the system gave one.
#[derive(Default)]
struct Exit(
    Mutex<Option<String>>,
    Condvar,
    AtomicBool,
    watch::Sender<bool>,
    tokio::sync::Notify,
    std::sync::OnceLock<u32>,
);

impl Exit {
    /// Whether the process has ended and been waited for.
    fn ended(&self) -> bool {
        *self.3.borrow()
    }

    /// Resolves once the process has ended and been waited for.
    async fn gone(&self) {
        let mut gone = self.3.subscribe();
        let _ = gone.wait_for(|gone| *gone).await;
    }

    /// Kill the process now, without waiting for it to end by itself. A
    /// kill asked for before the task that waits for it runs is kept.
    fn kill(&self) {
        self.4.notify_one();
    }

    fn record(&self, status: String) {
        self.0.lock().unwrap().get_or_insert(status);
        self.1.notify_all();
    }

    /// Waits for the process to end, so the channel's end can say how,
    /// however long that takes (#246): a process that closed its channel is
    /// ending, one whose channel was closed here is killed after [`GRACE`].
    /// Only one that keeps running with its channel closed reaches `guard`.
    fn wait(&self, guard: Duration) -> Option<String> {
        self.1
            .wait_timeout_while(self.0.lock().unwrap(), guard, |status| status.is_none())
            .unwrap()
            .0
            .clone()
    }

    /// This side closed the channel: wakes a send waiting for the process.
    fn closed_here(&self) {
        let status = self.0.lock().unwrap();
        self.2.store(true, Ordering::SeqCst);
        drop(status);
        self.1.notify_all();
    }

    /// [`Exit::wait`], unless or until this side closed the channel. A send
    /// holds the session's writer, which closing the session needs: the
    /// kill that would end the wait runs on the async runtime, which a
    /// synchronous call may be blocking.
    fn wait_unless_closed(&self, guard: Duration) -> Option<String> {
        let closed = || self.2.load(Ordering::SeqCst);
        let (status, _) = self
            .1
            .wait_timeout_while(self.0.lock().unwrap(), guard, |status| {
                status.is_none() && !closed()
            })
            .unwrap();
        status.clone()
    }
}

/// The processes [`spawn`](super::spawn) started into it, to
/// tell when every one of them has ended: what started them waits for its
/// own, not for whichever process ends first (#448).
#[derive(Clone, Default)]
pub struct Processes(Arc<Mutex<Vec<Arc<Exit>>>>);

impl Processes {
    fn add(&self, exit: Arc<Exit>) {
        let mut started = self.0.lock().unwrap();
        started.retain(|exit| !exit.ended());
        started.push(exit);
    }

    /// The process id of the last one started, while it has not ended.
    pub fn pid(&self) -> Option<u32> {
        let started = self.0.lock().unwrap();
        started
            .last()
            .filter(|exit| !exit.ended())
            .and_then(|exit| exit.5.get().copied())
    }

    /// How the last one started ended (`exited normally`, `exited with exit
    /// status: 1`), once it has.
    pub fn status(&self) -> Option<String> {
        let started = self.0.lock().unwrap();
        started
            .last()
            .and_then(|exit| exit.0.lock().unwrap().clone())
    }

    /// Whether one of them has not ended yet.
    pub fn running(&self) -> bool {
        let mut started = self.0.lock().unwrap();
        started.retain(|exit| !exit.ended());
        !started.is_empty()
    }

    /// Kill those that have not ended yet, now: for a runtime that did not
    /// stop within its deadline. [`Processes::ended`] then waits for them.
    pub fn kill(&self) {
        for exit in self.0.lock().unwrap().iter() {
            exit.kill();
        }
    }

    /// Resolves once every one of them has ended and been waited for.
    pub async fn ended(&self) {
        let started: Vec<Arc<Exit>> = self.0.lock().unwrap().clone();
        for exit in started {
            exit.gone().await;
        }
    }
}

/// How long a process whose channel closed may take to end by itself.
const GRACE: Duration = Duration::from_secs(2);

/// How long a process whose channel ended may go on running before the
/// channel ends without its exit status. Not a wait for the status: a
/// process that is exiting may close its channel well before it is gone,
/// and under load that took more than the grace period this once was
/// (#243, #246).
const HANG_GUARD: Duration = Duration::from_secs(10);

/// The processes started here that have not been waited for yet.
static RUNNING: std::sync::LazyLock<watch::Sender<BTreeSet<u32>>> =
    std::sync::LazyLock::new(|| watch::channel(BTreeSet::new()).0);

/// The ids of the processes [`spawn`](super::spawn) started, in this process,
/// that have not ended yet.
pub fn running_processes() -> Vec<u32> {
    RUNNING.borrow().iter().copied().collect()
}

/// Wait until every process [`spawn`](super::spawn) started has ended and been
/// waited for: what a host that is ending does once its root has shut down
/// (the processes end when their channels close, or are killed after a
/// grace period). Needs the async runtime that started them to keep running.
pub async fn processes_ended() {
    let mut running = RUNNING.subscribe();
    let _ = running.wait_for(|running| running.is_empty()).await;
}

/// Kill every process [`spawn`](super::spawn) started that has not ended yet,
/// without waiting for its cleanups: for a host that exits before they
/// finish. [`processes_ended`] then waits for them to be gone.
pub fn kill_processes() {
    // Manifest readers are in no channel and no job, and may be between
    // reads or hanging: kill them and what they started.
    kill_manifests();
    // On Windows, every one of them is in this process's job, with what it
    // started: end the job. Elsewhere, each is killed by the task that
    // waits for it, so never once it has been waited for and its id reused.
    #[cfg(windows)]
    if let Some(Ok(job)) = JOB.get() {
        // SAFETY: the job handle lives as long as this process.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(*job as _, 1);
        }
    }
    #[cfg(not(windows))]
    KILL.send_modify(|kills| *kills += 1);
}

/// Bumped to kill every process not waited for yet.
#[cfg(not(windows))]
static KILL: std::sync::LazyLock<watch::Sender<u64>> =
    std::sync::LazyLock::new(|| watch::channel(0).0);

/// Kill `child` now. On Unix, while it has not been waited for, its id
/// still names it and the group it leads: a process in a group of its own
/// takes what it started with it.
fn kill_now(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let (Ok(None), Some(pid)) = (child.try_wait(), child.id()) {
        let pid = pid as libc::pid_t;
        // SAFETY: plain getpgid(2) and kill(2) on this process's child,
        // which `try_wait` just found not waited for: neither its id nor
        // its group's can have been reused.
        unsafe {
            if libc::getpgid(pid) == pid {
                libc::kill(-pid, libc::SIGKILL);
                return;
            }
        }
    }
    let _ = child.start_kill();
}

/// The process, owned by a task that records how it ended. Dropped, it
/// kills the process; [`Child::end`] gives it [`GRACE`] first.
struct Child {
    ended: Arc<Exit>,
    end: Mutex<Option<oneshot::Sender<()>>>,
}

impl Child {
    fn watch(mut child: tokio::process::Child) -> Self {
        let (kill, killed) = oneshot::channel::<()>();
        let ended = Arc::new(Exit::default());
        let pid = child.id();
        if let Some(pid) = pid {
            let _ = ended.5.set(pid);
            RUNNING.send_modify(|running| {
                running.insert(pid);
            });
            let record = ended.clone();
            let _ = std::thread::Builder::new()
                .name("rutis-spawn-exit".into())
                .spawn(move || {
                    if let Some(status) = peek_exit(pid) {
                        record.record(describe(Ok(status)));
                    }
                });
        }
        let record = ended.clone();
        #[cfg(not(windows))]
        let mut kills = KILL.subscribe();
        tokio::spawn(async move {
            #[cfg(not(windows))]
            let killed_all = async {
                let _ = kills.changed().await;
            };
            #[cfg(windows)]
            let killed_all = std::future::pending::<()>();
            tokio::pin!(killed_all);
            let killed_now = record.4.notified();
            tokio::pin!(killed_now);
            let status = tokio::select! {
                status = child.wait() => status,
                () = &mut killed_all => {
                    kill_now(&mut child);
                    child.wait().await
                }
                () = &mut killed_now => {
                    kill_now(&mut child);
                    child.wait().await
                }
                ended = killed => {
                    // Its channel closed: it may end by itself.
                    let by_itself = match ended {
                        Ok(()) => tokio::select! {
                            status = tokio::time::timeout(GRACE, child.wait()) => status.ok(),
                            () = &mut killed_all => None,
                            () = &mut killed_now => None,
                        },
                        Err(_) => None,
                    };
                    match by_itself {
                        Some(status) => status,
                        None => {
                            kill_now(&mut child);
                            child.wait().await
                        }
                    }
                }
            };
            record.record(describe(status));
            // Waited for: the process is gone, its id free for reuse.
            if let Some(pid) = pid {
                RUNNING.send_modify(|running| {
                    running.remove(&pid);
                });
            }
            record.3.send_replace(true);
        });
        Self {
            ended,
            end: Mutex::new(Some(kill)),
        }
    }

    /// End the process: its channel closed, so it should end by itself;
    /// it is killed if it has not after [`GRACE`].
    fn end(&self) {
        if let Some(end) = self.end.lock().unwrap().take() {
            let _ = end.send(());
        }
    }
}

fn describe(status: std::io::Result<std::process::ExitStatus>) -> String {
    match status {
        Ok(status) if status.success() => "exited normally".to_owned(),
        Ok(status) => format!("exited with {status}"),
        Err(error) => format!("cannot be waited for: {error}"),
    }
}

/// Waits until the process `pid` ends and reports its status without reaping
/// it (tokio still does), independently of any runtime.
#[cfg(unix)]
fn peek_exit(pid: u32) -> Option<std::process::ExitStatus> {
    use std::os::unix::process::ExitStatusExt;
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `info` is a valid, writable siginfo_t.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if result == 0 {
            break;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return None; // already reaped: the runtime reports it
        }
    }
    // SAFETY: waitid filled a SIGCHLD siginfo_t.
    let status = unsafe { info.si_status() };
    // Rebuild the raw wait status so it formats like `ExitStatus`.
    let raw = match info.si_code {
        libc::CLD_EXITED => (status & 0xff) << 8,
        libc::CLD_DUMPED => status | 0x80,
        _ => status,
    };
    Some(std::process::ExitStatus::from_raw(raw))
}

/// Waits until the process `pid` ends and reports its exit code, through a
/// handle of its own, independently of any runtime.
#[cfg(windows)]
pub(crate) fn peek_exit(pid: u32) -> Option<std::process::ExitStatus> {
    use std::os::windows::process::ExitStatusExt;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, WaitForSingleObject, INFINITE,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };
    // SAFETY: plain Win32 calls on a handle this function owns and closes.
    unsafe {
        let handle = OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if handle.is_null() {
            return None;
        }
        let mut code = 0u32;
        let ended = WaitForSingleObject(handle, INFINITE) == WAIT_OBJECT_0
            && GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        ended.then(|| std::process::ExitStatus::from_raw(code))
    }
}

#[cfg(not(any(unix, windows)))]
fn peek_exit(_pid: u32) -> Option<std::process::ExitStatus> {
    None
}

/// The job [`adopt`] puts processes in.
#[cfg(windows)]
static JOB: std::sync::OnceLock<Result<usize, String>> = std::sync::OnceLock::new();

/// On Windows, put the process in a job that is closed, ending every
/// process in it, when this process ends however it does: a runtime does
/// not outlive its host. A process that cannot be put in it is an error.
/// Elsewhere, the process ends when its channel closes.
#[cfg(windows)]
pub(crate) fn adopt(child: &tokio::process::Child) -> Result<(), String> {
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
    let job = JOB
        .get_or_init(|| job().map(|handle| handle as usize))
        .clone()?;
    let process = child
        .raw_handle()
        .ok_or_else(|| "the process has no handle".to_owned())?;
    // SAFETY: both handles are live: the job for this process's lifetime,
    // the child's while it is owned.
    let assigned = unsafe { AssignProcessToJobObject(job as _, process as _) };
    match assigned {
        0 => Err(format!(
            "cannot assign it to a job: {}",
            std::io::Error::last_os_error()
        )),
        _ => Ok(()),
    }
}

/// A job that ends its processes when its last handle closes: the one this
/// process holds until it ends.
#[cfg(windows)]
fn job() -> Result<windows_sys::Win32::Foundation::HANDLE, String> {
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    // SAFETY: plain Win32 calls; the job handle is kept for the process's
    // lifetime.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(format!(
                "cannot create a job: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        match set {
            0 => Err(format!(
                "cannot set up a job: {}",
                std::io::Error::last_os_error()
            )),
            _ => Ok(job),
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn adopt(_child: &tokio::process::Child) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct Ends;
    impl Receiver for Ends {
        fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
            Ok(None)
        }
    }
    struct Broken;
    impl Sender for Broken {
        fn send(&mut self, _: &[u8]) -> Result<(), ChannelError> {
            Err(ChannelError::Closed {
                reason: "Broken pipe".into(),
            })
        }
    }

    /// #246: a process may close its channel well before it exits, under
    /// load longer than the grace period and the second the channel's end
    /// once waited. The channel's end, and a send after it, wait for the
    /// exit however long it takes, and say how the process ended.
    #[test]
    fn the_end_of_the_channel_waits_for_the_exit_status() {
        let exit = Arc::new(Exit::default());
        let mut receiver = Ended {
            receiver: Box::new(Ends),
            ended: Some(exit.clone()),
        };
        let mut sender = EndedSender {
            sender: Box::new(Broken),
            ended: exit.clone(),
        };
        let (received, receiving) = mpsc::channel();
        std::thread::spawn(move || received.send(receiver.recv().map(|_| ())).unwrap());
        let (sent, sending) = mpsc::channel();
        std::thread::spawn(move || sent.send(sender.send(b"{}")).unwrap());

        let past = GRACE + Duration::from_millis(1500);
        assert!(
            receiving.recv_timeout(past).is_err(),
            "the channel ended first"
        );
        assert!(sending.try_recv().is_err(), "the send failed first");
        exit.record("exited with exit status: 17".into());
        for (end, ended) in [
            ("recv", receiving.recv().unwrap()),
            ("send", sending.recv().unwrap()),
        ] {
            match ended {
                Err(ChannelError::Closed { reason }) => {
                    assert_eq!(reason, "the process exited with exit status: 17", "{end}")
                }
                other => panic!("the {end} should fail with the exit status, got {other:?}"),
            }
        }
    }

    /// Closed here, a failed send fails at once: it does not wait for the
    /// process to be killed.
    #[test]
    fn a_send_after_closing_here_does_not_wait() {
        let exit = Arc::new(Exit::default());
        let mut sender = EndedSender {
            sender: Box::new(Broken),
            ended: exit.clone(),
        };
        let (sent, sending) = mpsc::channel();
        std::thread::spawn(move || sent.send(sender.send(b"{}")).unwrap());
        assert!(
            sending.recv_timeout(Duration::from_millis(200)).is_err(),
            "the send waits for the process"
        );
        exit.closed_here();
        match sending
            .recv_timeout(HANG_GUARD / 2)
            .expect("woken by the close")
        {
            Err(ChannelError::Closed { reason }) => assert_eq!(reason, "Broken pipe"),
            other => panic!("the send should fail with its own error, got {other:?}"),
        }
    }

    /// What the framing refuses itself fails at once, without waiting for
    /// a process that is still running.
    #[test]
    fn a_refused_message_does_not_wait() {
        let mut sender = EndedSender {
            sender: Box::new(Broken),
            ended: Arc::new(Exit::default()),
        };
        assert!(sender.send(b"a\nb").is_err());
    }
}

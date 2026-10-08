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
use std::process::Stdio;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::channel::{
    Channel, ChannelError, ChannelInfo, Closer, ConnectError, PeerId, Receiver, Sender,
};
use tokio::sync::oneshot;

use crate::transport::local::lines;

/// The fd a process finds its channel on (`fd:3`).
pub const CHANNEL_FD: i32 = 3;

/// The environment variable holding the token a process started with
/// [`Handover::Loopback`] presents, as its first line, on the address it is
/// given (`tcp:127.0.0.1:<port>`). It should remove it from its own
/// environment once read.
pub const CHANNEL_TOKEN: &str = "RUTIS_CHANNEL_TOKEN";

/// A process `spawn:<name>` starts: `program args… <channel> trailing…`,
/// where `<channel>` is `fd:3` or the socket path to dial.
#[derive(Clone, Debug)]
pub struct Spawn {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    /// The working directory; the application's by default.
    pub cwd: Option<PathBuf>,
    pub handover: Handover,
    /// Arguments after the channel.
    pub trailing: Vec<OsString>,
    /// The endpoint the process is: whoever starts a process names it.
    pub peer: PeerId,
}

/// How the process gets its channel. On Windows, every process gets
/// [`Handover::Loopback`]; elsewhere too when [`HANDOVER_VARIABLE`] is
/// `loopback` (to run Windows' path on Unix).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    pub fn new(program: impl Into<OsString>, peer: PeerId) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            handover: Handover::Inherit,
            trailing: Vec::new(),
            peer,
        }
    }

    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&self.args)
            .envs(self.env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
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
pub(crate) async fn start(spawn: &Spawn) -> Result<Channel, ConnectError> {
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
        mut info,
    } = channel;
    info.peer = Some(spawn.peer.clone());
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
        info,
    })
}

type Started = (Channel, Child, Option<tempfile::TempDir>);

#[cfg(unix)]
fn inherit(spawn: &Spawn) -> Result<Started, ConnectError> {
    let (ours, theirs) = UnixStream::pair().map_err(retryable)?;
    let fd = theirs.as_raw_fd();
    let mut command = spawn.command();
    // SAFETY: between fork and exec, only async-signal-safe calls: dup2 and
    // fcntl. Our end and every other descriptor keep CLOEXEC; only fd 3
    // crosses exec.
    unsafe {
        command.pre_exec(move || {
            if fd == CHANNEL_FD {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(fd, CHANNEL_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command
        .arg(format!("fd:{CHANNEL_FD}"))
        .args(&spawn.trailing)
        .spawn()
        .map_err(|error| cannot_start(spawn, error))?;
    // The child has its copy; ours would keep the channel open after it exits.
    drop(theirs);
    Ok((socket(ours, "fd")?, Child::watch(child), None))
}

#[cfg(unix)]
async fn dial_back(spawn: &Spawn) -> Result<Started, ConnectError> {
    let directory = tempfile::Builder::new()
        .prefix("rutis-spawn-")
        .tempdir()
        .map_err(retryable)?;
    let path = directory.path().join("peer.sock");
    let listener = tokio::net::UnixListener::bind(&path).map_err(retryable)?;
    let mut child = spawn
        .command()
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
    Ok((
        socket(stream, "unix")?,
        Child::watch(child),
        Some(directory),
    ))
}

/// How long a connection to the loopback listener may take to present its
/// token.
const TOKEN_WAIT: Duration = Duration::from_secs(10);

async fn loopback(spawn: &Spawn) -> Result<Started, ConnectError> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(retryable)?;
    let address = listener.local_addr().map_err(retryable)?;
    let token = token().map_err(retryable)?;
    let mut child = spawn
        .command()
        .env(CHANNEL_TOKEN, &token)
        .arg(format!("tcp:{address}"))
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
    let stream = loop {
        let mut stream = tokio::select! {
            accepted = listener.accept() => accepted.map_err(retryable)?.0,
            status = child.wait() => return Err(ConnectError::Retryable {
                reason: format!("the process exited before connecting: {}", describe(status)),
            }),
        };
        // Anyone on this machine can connect: only the process knows the
        // token.
        let presented = tokio::time::timeout(TOKEN_WAIT, read_line(&mut stream)).await;
        if matches!(presented, Ok(Ok(line)) if line == token) {
            break stream;
        }
    };
    let stream = stream.into_std().map_err(retryable)?;
    stream.set_nonblocking(false).map_err(retryable)?;
    let _ = stream.set_nodelay(true);
    let reader = stream.try_clone().map_err(retryable)?;
    let closer = Arc::new(ShutTcp(stream.try_clone().map_err(retryable)?));
    let channel = lines::channel(
        reader,
        stream,
        closer,
        ChannelInfo {
            transport: "tcp",
            peer: None,
            label: String::new(),
        },
    );
    Ok((channel, Child::watch(child), None))
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

struct ShutTcp(std::net::TcpStream);

impl Closer for ShutTcp {
    fn close(&self, _reason: &str) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
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
fn socket(stream: UnixStream, transport: &'static str) -> Result<Channel, ConnectError> {
    stream.set_nonblocking(false).map_err(retryable)?;
    let reader = stream.try_clone().map_err(retryable)?;
    let closer = Arc::new(crate::transport::local::unix::Shut(
        stream.try_clone().map_err(retryable)?,
    ));
    Ok(lines::channel(
        reader,
        stream,
        closer,
        ChannelInfo {
            transport,
            peer: None,
            label: String::new(),
        },
    ))
}

/// A closer that owns its process: dropped, it ends the process.
struct Owning {
    closer: Arc<dyn Closer>,
    child: Child,
    _directory: Option<tempfile::TempDir>,
}

impl Closer for Owning {
    fn close(&self, reason: &str) {
        self.closer.close(reason);
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
                reason: match self.ended.take().and_then(|ended| ended.wait()) {
                    Some(status) => format!("the process {status}"),
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
        self.sender
            .send(message)
            .map_err(|error| match self.ended.wait() {
                Some(status) => ChannelError::Closed {
                    reason: format!("the process {status}"),
                },
                None => error,
            })
    }
}

/// How the process ended, once it has. Readable from the reader thread,
/// which must not depend on the async runtime: a current-thread runtime may
/// be blocked in a synchronous call.
#[derive(Default)]
struct Exit(Mutex<Option<String>>, Condvar);

impl Exit {
    fn record(&self, status: String) {
        self.0.lock().unwrap().get_or_insert(status);
        self.1.notify_all();
    }

    /// Waits for the process to end, so the channel's end can say how: a
    /// process that closed its channel ends soon, one whose channel was
    /// closed within the grace period.
    fn wait(&self) -> Option<String> {
        let wait = GRACE + Duration::from_secs(1);
        self.1
            .wait_timeout_while(self.0.lock().unwrap(), wait, |status| status.is_none())
            .unwrap()
            .0
            .clone()
    }
}

/// How long a process whose channel closed may take to end by itself.
const GRACE: Duration = Duration::from_secs(2);

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
        if let Some(pid) = child.id() {
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
        tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status,
                ended = killed => {
                    // Its channel closed: it may end by itself.
                    if ended.is_ok() {
                        if let Ok(status) = tokio::time::timeout(GRACE, child.wait()).await {
                            record.record(describe(status));
                            return;
                        }
                    }
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            record.record(describe(status));
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
fn peek_exit(pid: u32) -> Option<std::process::ExitStatus> {
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

/// On Windows, put the process in a job that is closed, ending every
/// process in it, when this process ends however it does: a runtime does
/// not outlive its host. A process that cannot be put in it is an error.
/// Elsewhere, the process ends when its channel closes.
#[cfg(windows)]
fn adopt(child: &tokio::process::Child) -> Result<(), String> {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
    static JOB: OnceLock<Result<usize, String>> = OnceLock::new();
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
fn adopt(_child: &tokio::process::Child) -> Result<(), String> {
    Ok(())
}

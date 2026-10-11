//! How the `Process` compatibility facade starts its runtime process: on an
//! inherited socket (fd 3) or, for runtimes that cannot take one, on a Unix
//! socket in a private directory that the process dials back; on Windows
//! (or with `RUTIS_LOCAL_HANDOVER=loopback`), on a loopback address with a
//! one-time token, as the local transport does. How it ends is watched and
//! reported as the session's end.
//!
//! Runtimes that get their sessions through a link are started by the local
//! transport ([`crate::transport::local`]), which has its own spawner; this
//! copy goes with the facade.
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::channel::{Channel, ChannelError, Closer, Receiver, Sender};
use tokio::sync::{oneshot, watch};

use crate::runtime::Error;

/// The fd a runtime process finds its channel on (`fd:3`).
#[cfg(unix)]
const CHANNEL_FD: i32 = 3;

/// How the process gets its channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Connect {
    /// One end of a socket pair, as fd 3: `fd:3 <first>`.
    Inherit,
    /// A socket path the process dials: `<path> <first>`.
    DialBack,
    /// A loopback address the process dials, presenting a token first:
    /// `tcp:<host>:<port> <first>`.
    Loopback,
}

/// A started runtime process and the channel it connected.
pub(crate) struct Spawned {
    pub channel: Channel,
    pub child: Child,
    /// Holds the socket when the process dials back; removed when the
    /// process is dropped.
    pub directory: Option<tempfile::TempDir>,
}

#[cfg(unix)]
fn transport(error: std::io::Error) -> Error {
    Error::Transport(error.to_string())
}

/// Start `command` with the channel and `first` (the first plugin or the
/// anchor) as its last two arguments, and connect it. The channel ends with
/// how the process ended (`Cordis process exited …`).
pub(crate) async fn spawn(
    command: tokio::process::Command,
    first: &Path,
    connect: Connect,
) -> Result<Spawned, Error> {
    match connect {
        #[cfg(unix)]
        Connect::Inherit => inherit(command, first),
        #[cfg(unix)]
        Connect::DialBack => dial_back(command, first).await,
        _ => loopback(command, first).await,
    }
}

async fn loopback(mut command: tokio::process::Command, first: &Path) -> Result<Spawned, Error> {
    use crate::transport::local::spawn::{adopt, Loopback, CHANNEL_TOKEN};
    let transport = |error: std::io::Error| Error::Transport(error.to_string());
    let listener = Loopback::bind().await.map_err(transport)?;
    #[cfg(unix)]
    crate::transport::local::spawn::hand_over(&mut command, None);
    let mut child = command
        .env(CHANNEL_TOKEN, listener.token())
        .arg(listener.address())
        .arg(first)
        .kill_on_drop(true)
        .spawn()
        .map_err(transport)?;
    // A process that would outlive its host is not started.
    if let Err(reason) = adopt(&child) {
        let _ = child.start_kill();
        return Err(Error::Transport(format!(
            "cannot tie the runtime process to this process's lifetime: {reason}"
        )));
    }
    let channel = tokio::select! {
        channel = listener.accept() => channel.map_err(transport)?,
        status = child.wait() => return Err(Error::Transport(match status {
            Ok(status) => format!("Cordis process exited before connecting: {status}"),
            Err(error) => format!("Cordis process exited before connecting: {error}"),
        })),
    };
    let child = Child::watch(child);
    let channel = on_exit(channel, &child);
    Ok(Spawned {
        channel: traced(channel),
        child,
        directory: None,
    })
}

/// Ends the channel, however it ends, with the error `disconnected` builds.
struct Disconnected {
    receiver: Box<dyn Receiver>,
    disconnected: Option<Box<dyn FnOnce() -> Error + Send>>,
}

impl Receiver for Disconnected {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
        match self.receiver.recv() {
            Ok(Some(message)) => Ok(Some(message)),
            _ => Err(ChannelError::Closed {
                reason: match self.disconnected.take() {
                    Some(disconnected) => disconnected().to_string(),
                    None => "peer disconnected".into(),
                },
            }),
        }
    }
}

/// Replace how the channel reports its end: `disconnected` runs on the
/// reader thread once the far end is gone, and may block.
pub(crate) fn on_disconnect(
    mut channel: Channel,
    disconnected: Box<dyn FnOnce() -> Error + Send>,
) -> Channel {
    channel.receiver = Box::new(Disconnected {
        receiver: channel.receiver,
        disconnected: Some(disconnected),
    });
    channel
}

/// A send to a process that left says how it ended, as the receiver does.
/// Its failure comes from the same end, often before the receiver has seen
/// it, and would otherwise end the session first with its own error
/// ("the channel ended", a broken pipe) (#250).
///
/// The local transport's `EndedSender` does the same for the runtimes it
/// starts. Its closer ends the process, so its wait ends either way; this
/// closer does not, so a close from this side, and a message the framing
/// refuses, fail at once here instead of waiting for the process.
struct ExitSender {
    sender: Box<dyn Sender>,
    ended: Arc<(Mutex<Option<String>>, Condvar)>,
    closed: Arc<AtomicBool>,
}

impl Sender for ExitSender {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        self.sender.send(message).map_err(|error| {
            // Refused by the framing (over its limit, a raw newline):
            // nothing the process did.
            if crate::transport::local::lines::refuses(message) {
                return error;
            }
            // Closed here, before or while waiting: the session ends for
            // that.
            match ended_unless(&self.ended, HANG_GUARD, || {
                self.closed.load(Ordering::SeqCst)
            }) {
                Some(ended) => ChannelError::Closed {
                    reason: ended.to_string(),
                },
                None => error,
            }
        })
    }
}

/// Notes that this side closed the channel, for [`ExitSender`], waking a
/// send that waits for the process.
struct ClosedHere {
    closer: Arc<dyn Closer>,
    ended: Arc<(Mutex<Option<String>>, Condvar)>,
    closed: Arc<AtomicBool>,
}

impl ClosedHere {
    fn note(&self) {
        let (status, changed) = &*self.ended;
        let status = status.lock().unwrap();
        self.closed.store(true, Ordering::SeqCst);
        drop(status);
        changed.notify_all();
    }
}

impl Closer for ClosedHere {
    fn close(&self, reason: &str) {
        self.note();
        self.closer.close(reason);
    }

    fn replaced(&self) {
        self.note();
        self.closer.replaced();
    }
}

/// The channel of the process `child` started, ending either way with how
/// the process ended. It must frame as
/// [`lines`](crate::transport::local::lines) does, with its default limit.
fn on_exit(channel: Channel, child: &Child) -> Channel {
    let mut channel = on_disconnect(channel, Box::new(child.disconnected()));
    let closed = Arc::new(AtomicBool::new(false));
    channel.sender = Box::new(ExitSender {
        sender: channel.sender,
        ended: child.ended.clone(),
        closed: closed.clone(),
    });
    channel.closer = Arc::new(ClosedHere {
        closer: channel.closer,
        ended: child.ended.clone(),
        closed,
    });
    channel
}

#[cfg(unix)]
fn inherit(mut command: tokio::process::Command, first: &Path) -> Result<Spawned, Error> {
    let (ours, theirs) = UnixStream::pair().map_err(transport)?;
    // Only fd 3 (and stdio) crosses exec, whatever other threads opened.
    crate::transport::local::spawn::hand_over(&mut command, Some(theirs.as_raw_fd()));
    let child = command
        .arg(format!("fd:{CHANNEL_FD}"))
        .arg(first)
        .kill_on_drop(true)
        .spawn()
        .map_err(transport)?;
    // The child has its copy; ours would keep the channel open after it exits.
    drop(theirs);
    let child = Child::watch(child);
    let mut channel = crate::runtime::unix::channel(ours, "")?;
    channel.info.transport = "fd";
    let channel = on_exit(channel, &child);
    Ok(Spawned {
        channel: traced(channel),
        child,
        directory: None,
    })
}

#[cfg(unix)]
async fn dial_back(mut command: tokio::process::Command, first: &Path) -> Result<Spawned, Error> {
    let directory = tempfile::Builder::new()
        .prefix("rutis-mount-")
        .tempdir()
        .map_err(transport)?;
    let socket = directory.path().join("peer.sock");
    let listener = tokio::net::UnixListener::bind(&socket).map_err(transport)?;
    crate::transport::local::spawn::hand_over(&mut command, None);
    let mut child = command
        .arg(&socket)
        .arg(first)
        .kill_on_drop(true)
        .spawn()
        .map_err(transport)?;
    let stream = tokio::select! {
        accepted = listener.accept() => accepted.map_err(transport)?.0,
        status = child.wait() => return Err(Error::Transport(match status {
            Ok(status) => format!("Cordis process exited before connecting: {status}"),
            Err(error) => format!("Cordis process exited before connecting: {error}"),
        })),
    };
    let stream = stream.into_std().map_err(transport)?;
    let child = Child::watch(child);
    let channel = on_exit(crate::runtime::unix::channel(stream, "")?, &child);
    Ok(Spawned {
        channel: traced(channel),
        child,
        directory: Some(directory),
    })
}

/// Set to report every message crossing a runtime channel (direction and
/// length, never content) on stderr.
pub(crate) const TRACE_VARIABLE: &str = "RUTIS_TRACE";

fn traced(channel: Channel) -> Channel {
    match std::env::var_os(TRACE_VARIABLE) {
        Some(_) => crate::channel::trace::trace(
            channel,
            Arc::new(|line: &str| eprintln!("rutis trace: {line}")),
        ),
        None => channel,
    }
}

/// The command a runtime process starts with: `launcher`, or the Node
/// runtime of the npm package `node_package` (feature `node`), and how it
/// takes its channel.
pub(crate) fn command(
    launcher: Option<&crate::runtime::Launcher>,
    node_package: &Path,
) -> Result<(tokio::process::Command, Connect), Error> {
    #[cfg(feature = "node")]
    let node = crate::runtime::Launcher::node(node_package);
    let launcher = match launcher {
        Some(launcher) => launcher,
        #[cfg(feature = "node")]
        None => &node,
        #[cfg(not(feature = "node"))]
        None => {
            let _ = node_package;
            return Err(Error::Value(
                "no launcher given, and the Node runtime needs the `node` feature".into(),
            ));
        }
    };
    let mut command = tokio::process::Command::new(&launcher.program);
    command
        .args(&launcher.args)
        .envs(launcher.env.iter().map(|(name, value)| (name, value)))
        .stdin(launcher.stdin.process())
        .stdout(launcher.stdout.process())
        .stderr(launcher.stderr.process());
    // Without a directory of its own, it runs where the application does.
    if let Some(cwd) = &launcher.cwd {
        command.current_dir(cwd);
    }
    let loopback = !cfg!(unix)
        || std::env::var_os(crate::transport::local::HANDOVER_VARIABLE)
            .is_some_and(|value| value == "loopback");
    let connect = match (loopback, launcher.inherit_fd) {
        (true, _) => Connect::Loopback,
        (false, true) => Connect::Inherit,
        (false, false) => Connect::DialBack,
    };
    Ok((command, connect))
}

/// Whether the Node runtime package at `package` takes an inherited channel:
/// its `package.json` lists `"fd"` in `rutisChannels`. Older packages do
/// not, and dial back.
#[cfg(feature = "node")]
pub(crate) fn node_inherits(package: &Path) -> bool {
    let channels = std::fs::read(package.join("package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|manifest| manifest.get("rutisChannels").cloned());
    matches!(channels, Some(serde_json::Value::Array(channels)) if channels.iter().any(|c| c == "fd"))
}

/// The Node process, owned by a task that records how it ended. Dropping
/// `_kill` ends the process.
pub(crate) struct Child {
    exit: watch::Receiver<Option<String>>,
    /// The same status for the reader thread, which must not depend on the
    /// runtime: a current-thread runtime may be blocked in a synchronous call.
    ended: Arc<(Mutex<Option<String>>, Condvar)>,
    _kill: oneshot::Sender<()>,
}

impl Child {
    pub(crate) fn watch(mut child: tokio::process::Child) -> Self {
        let (kill, killed) = oneshot::channel::<()>();
        let (report, exit) = watch::channel(None);
        let ended = Arc::new((Mutex::new(None), Condvar::new()));
        if let Some(pid) = child.id() {
            let record = ended.clone();
            let _ = std::thread::Builder::new()
                .name("rutis-exit".into())
                .spawn(move || {
                    if let Some(status) = peek_exit(pid) {
                        record_exit(&record, describe(Ok(status)));
                    }
                });
        }
        let record = ended.clone();
        tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status,
                _ = killed => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            let status = describe(status);
            record_exit(&record, status.clone());
            report.send_replace(Some(status));
        });
        Self {
            exit,
            ended,
            _kill: kill,
        }
    }

    /// How the process ended, once it has: as soon as either the exit
    /// watcher or the runtime records it, so it agrees with the error that
    /// ended the session.
    pub(crate) fn status(&self) -> Option<String> {
        self.ended.0.lock().unwrap().clone()
    }

    pub(crate) async fn exited(&self) -> String {
        let mut exit = self.exit.clone();
        let status = exit.wait_for(Option::is_some).await;
        status.map_or_else(|_| "is gone".to_owned(), |status| status.clone().unwrap())
    }

    /// The error that ends a session whose peer went away: waits for the
    /// process to end so the error can say how.
    pub(crate) fn disconnected(&self) -> impl FnOnce() -> Error + Send {
        let ended = self.ended.clone();
        move || ended_with(&ended, HANG_GUARD)
    }
}

/// How long a process whose channel ended may go on running before the
/// session ends without its exit status. Only a process that keeps running
/// with its channel closed reaches it: one that is exiting gets there
/// eventually, and a process may close its channel well before it is gone
/// (Node tears down its I/O worker, and with it the socket, before exiting;
/// under load that took more than the second this once was, #233).
const HANG_GUARD: Duration = Duration::from_secs(10);

/// Waits for the process to end, as `ended` records it, and says how; a
/// process still running after `guard` is reported as such.
fn ended_with(ended: &(Mutex<Option<String>>, Condvar), guard: Duration) -> Error {
    ended_unless(ended, guard, || false).expect("not stopped")
}

/// [`ended_with`], or `None` as soon as `stop` holds instead: checked under
/// `ended`'s lock, so whoever makes it hold sets it under that lock and
/// notifies.
fn ended_unless(
    ended: &(Mutex<Option<String>>, Condvar),
    guard: Duration,
    stop: impl Fn() -> bool,
) -> Option<Error> {
    let (status, changed) = ended;
    let (status, _) = changed
        .wait_timeout_while(status.lock().unwrap(), guard, |status| {
            status.is_none() && !stop()
        })
        .unwrap();
    if status.is_none() && stop() {
        return None;
    }
    Some(Error::Transport(match status.clone() {
        Some(status) => format!("Cordis process {status}"),
        None => format!(
            "Cordis process closed its channel but has not exited after {}s",
            guard.as_secs_f32()
        ),
    }))
}

fn describe(status: std::io::Result<std::process::ExitStatus>) -> String {
    match status {
        Ok(status) if status.success() => "exited normally".to_owned(),
        Ok(status) => format!("exited with {status}"),
        Err(error) => format!("cannot be waited for: {error}"),
    }
}

fn record_exit(ended: &(Mutex<Option<String>>, Condvar), status: String) {
    ended.0.lock().unwrap().get_or_insert(status);
    ended.1.notify_all();
}

/// Waits until the process `pid` ends and reports its status, independently
/// of any runtime.
#[cfg(windows)]
use crate::transport::local::spawn::peek_exit;

#[cfg(not(any(unix, windows)))]
fn peek_exit(_pid: u32) -> Option<std::process::ExitStatus> {
    None
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc;

    /// A shell that closes its channel (fd 3), then waits for a line on the
    /// fifo `gate` before exiting with 17: the channel ends while the
    /// process is still running, for as long as the test keeps it so.
    fn closes_then_waits(gate: &Path) -> tokio::process::Command {
        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg(r#"exec 3>&-; read line < "$GATE"; exit 17"#)
            .env("GATE", gate);
        command
    }

    fn fifo(directory: &Path) -> std::path::PathBuf {
        let path = directory.join("gate");
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `name` is a valid C string.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        path
    }

    /// #233: a process may close its channel well before it exits (Node
    /// closes it while tearing down). The channel's end waits for the exit,
    /// however long that takes, and says how the process ended.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_end_of_the_channel_waits_for_the_exit_status() {
        let directory = tempfile::tempdir().unwrap();
        let gate = fifo(directory.path());
        let Spawned { channel, child, .. } =
            inherit(closes_then_waits(&gate), Path::new("first")).unwrap();
        let mut receiver = channel.receiver;
        let (ended, end) = mpsc::channel();
        std::thread::spawn(move || ended.send(receiver.recv()).unwrap());

        // Past the second the channel's end used to wait for: still
        // waiting, since the process is still running.
        assert!(
            end.recv_timeout(Duration::from_secs(2)).is_err(),
            "the channel ended before the process did"
        );
        assert_eq!(child.status(), None);

        std::fs::OpenOptions::new()
            .write(true)
            .open(&gate)
            .unwrap()
            .write_all(b"exit\n")
            .unwrap();
        match end.recv().unwrap() {
            Err(ChannelError::Closed { reason }) => {
                assert_eq!(reason, "Cordis process exited with exit status: 17")
            }
            other => panic!("the channel should end with the exit status, got {other:?}"),
        }
        assert_eq!(child.exited().await, "exited with exit status: 17");
    }

    fn open_gate(gate: &Path) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(gate)
            .unwrap()
            .write_all(b"exit\n")
            .unwrap();
    }

    /// Sends until a send fails (for at most 5s): on macOS a socket whose
    /// far end closed still takes writes until the receiver has seen the end.
    fn send_until_it_fails(
        mut sender: Box<dyn Sender>,
    ) -> mpsc::Receiver<Result<(), ChannelError>> {
        let (sent, send) = mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..500 {
                if let Err(error) = sender.send(b"{}") {
                    let _ = sent.send(Err(error));
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        send
    }

    /// Long enough for anything here to settle, short of a hung suite.
    const SETTLED: Duration = Duration::from_secs(5);

    /// #250: a send that fails because the process closed its channel says
    /// how the process ended, as the receiver does, instead of ending the
    /// session first with its own error.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_send_after_the_channel_ended_says_how_the_process_ended() {
        let directory = tempfile::tempdir().unwrap();
        let gate = fifo(directory.path());
        let Spawned { channel, child, .. } =
            inherit(closes_then_waits(&gate), Path::new("first")).unwrap();
        let Channel {
            sender,
            mut receiver,
            ..
        } = channel;
        let (received, receive) = mpsc::channel();
        std::thread::spawn(move || received.send(receiver.recv()).unwrap());
        let send = send_until_it_fails(sender);

        assert!(
            send.recv_timeout(Duration::from_millis(500)).is_err(),
            "the send ended before the process did"
        );
        assert_eq!(child.status(), None);
        open_gate(&gate);
        let expected = ChannelError::Closed {
            reason: "Cordis process exited with exit status: 17".into(),
        };
        assert_eq!(send.recv_timeout(SETTLED).unwrap(), Err(expected.clone()));
        assert_eq!(receive.recv_timeout(SETTLED).unwrap(), Err(expected));
    }

    /// A send waiting for the process stops waiting once this side closes
    /// the channel, and fails for that rather than for how the process
    /// ended.
    #[tokio::test(flavor = "multi_thread")]
    async fn closing_here_releases_a_waiting_send() {
        let directory = tempfile::tempdir().unwrap();
        let gate = fifo(directory.path());
        let Spawned { channel, child, .. } =
            inherit(closes_then_waits(&gate), Path::new("first")).unwrap();
        let Channel {
            sender,
            mut receiver,
            closer,
            ..
        } = channel;
        std::thread::spawn(move || receiver.recv());
        let send = send_until_it_fails(sender);
        assert!(
            send.recv_timeout(Duration::from_millis(500)).is_err(),
            "the send waits for the process"
        );

        closer.close("closed here");
        let error = send
            .recv_timeout(Duration::from_secs(1))
            .expect("the close releases the send")
            .unwrap_err();
        assert!(!error.to_string().contains("exit status"), "{error}");
        assert_eq!(child.status(), None);
        open_gate(&gate);
        child.exited().await;
    }

    /// A send after this side closed the channel fails for that, not for
    /// how the process ended, and does not wait for the process.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_send_after_closing_here_fails_at_once() {
        let directory = tempfile::tempdir().unwrap();
        let gate = fifo(directory.path());
        let Spawned {
            mut channel, child, ..
        } = inherit(closes_then_waits(&gate), Path::new("first")).unwrap();
        channel.closer.close("closed here");
        let sending = std::time::Instant::now();
        let error = channel.sender.send(b"{}").unwrap_err();
        assert!(
            sending.elapsed() < Duration::from_secs(1),
            "the send waited"
        );
        assert!(!error.to_string().contains("exit status"), "{error}");
        assert_eq!(child.status(), None);
        open_gate(&gate);
        child.exited().await;
    }

    /// A message the framing refuses (a raw newline, over its limit)
    /// fails at once with that reason, whatever the process does.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_message_the_framing_refuses_fails_at_once() {
        let directory = tempfile::tempdir().unwrap();
        let gate = fifo(directory.path());
        let Spawned {
            mut channel, child, ..
        } = inherit(closes_then_waits(&gate), Path::new("first")).unwrap();
        let sending = std::time::Instant::now();
        let newline = channel.sender.send(b"a\nb").unwrap_err();
        assert_eq!(newline.to_string(), "message contains a raw newline");
        let over = channel
            .sender
            .send(&vec![b'x'; crate::transport::local::lines::MAX_MESSAGE + 1])
            .unwrap_err();
        assert!(over.to_string().contains("exceeds the limit"), "{over}");
        assert!(sending.elapsed() < Duration::from_secs(1), "a send waited");
        open_gate(&gate);
        child.exited().await;
    }

    /// A process that closed its channel and does not exit ends the session
    /// once the guard runs out, saying so rather than `peer disconnected`.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_process_that_does_not_exit_is_reported_as_such() {
        let directory = tempfile::tempdir().unwrap();
        let gate = fifo(directory.path());
        let Spawned { child, .. } = inherit(closes_then_waits(&gate), Path::new("first")).unwrap();
        let ended = child.ended.clone();
        let error =
            tokio::task::spawn_blocking(move || ended_with(&ended, Duration::from_millis(100)))
                .await
                .unwrap();
        assert_eq!(
            error.to_string(),
            "Cordis process closed its channel but has not exited after 0.1s"
        );
    }
}

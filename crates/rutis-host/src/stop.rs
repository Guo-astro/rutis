//! How `run` and `dev` end: on a termination signal the host shuts its
//! root down, so every plugin's cleanup runs once, and waits for that up to
//! a deadline; a second signal, or the deadline, ends it at once.

use std::time::Duration;

use rutis_host::errln;

use crate::host::{Host, Unfinished, PROCESS_EXIT};

/// How long cleanups may take after a signal, unless `--shutdown-timeout`
/// or [`DEADLINE_VARIABLE`] says otherwise.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(10);

/// Sets the deadline, in seconds, when the command line does not.
pub const DEADLINE_VARIABLE: &str = "RUTIS_SHUTDOWN_TIMEOUT";

const FLAG: &str = "--shutdown-timeout";

/// The exit code when cleanups did not finish: the deadline passed, or a
/// second signal ended the host first. Distinct from 1 (the host could not
/// start or run), and below 126, which shells use for a program they could
/// not run (126, 127) or one a signal ended (128 + n).
pub const UNFINISHED: i32 = 2;

/// The exit code when a local runtime whose `restart` is `exit` ended on
/// its own, and the host stopped (every cleanup finished): for a process
/// manager to restart the whole host. Distinct from 1 (it could not start
/// or run) and 2 (stopping did not finish, which wins over it).
pub const RUNTIME_ENDED: i32 = 3;

/// How the host ended after a signal.
pub enum Stopped {
    /// Every cleanup finished.
    Done,
    /// It could not start (the error is printed), and every cleanup of
    /// what did start finished; the host exits with 1.
    Failed,
    /// A runtime whose `restart` is `exit` ended, and every cleanup
    /// finished; the host exits with [`RUNTIME_ENDED`].
    RuntimeEnded,
    /// Cleanups did not finish; the host exits with [`UNFINISHED`].
    Unfinished,
}

impl Stopped {
    /// After starting failed: [`Stopped::Failed`] unless stopping did not
    /// finish either, which the exit code says first.
    pub fn failed(self) -> Self {
        match self {
            Stopped::Done => Stopped::Failed,
            other => other,
        }
    }
}

/// What ends `run` or `dev` once running.
pub enum Ended {
    /// A termination signal: its name.
    Signal(&'static str),
    /// A local runtime whose `restart` is `exit` ended: its name, and why.
    Runtime(String, String),
}

/// Take `--shutdown-timeout <seconds>` out of `args`; the deadline it, or
/// [`DEADLINE_VARIABLE`], gives.
pub fn deadline(args: &[String]) -> Result<(Duration, Vec<String>), String> {
    deadline_in(args, std::env::var(DEADLINE_VARIABLE).ok())
}

/// [`deadline`], with the variable's value `variable`.
fn deadline_in(
    args: &[String],
    variable: Option<String>,
) -> Result<(Duration, Vec<String>), String> {
    let mut given = None;
    let mut rest = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == FLAG {
            given = Some(
                args.next()
                    .ok_or_else(|| format!("{FLAG} needs a number of seconds"))?
                    .clone(),
            );
        } else if let Some(value) = arg.strip_prefix(&format!("{FLAG}=")) {
            given = Some(value.to_owned());
        } else {
            rest.push(arg.clone());
        }
    }
    let (source, value) = match given {
        Some(value) => (FLAG, value),
        None => match variable {
            Some(value) => (DEADLINE_VARIABLE, value),
            None => return Ok((DEFAULT_DEADLINE, rest)),
        },
    };
    let deadline = value
        .trim()
        .parse::<f64>()
        .ok()
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .filter(|deadline| !deadline.is_zero())
        .ok_or_else(|| format!("{source}: {value:?} is not a number of seconds above 0"))?;
    Ok((deadline, rest))
}

/// Give the runtime processes this host starts the same deadline, in
/// [`DEADLINE_VARIABLE`], which they inherit: a runtime whose host went
/// away without unloading it (killed) unloads its rows itself, gives up
/// the cleanups that did not finish by then, and exits.
pub fn pass_on(deadline: Duration) {
    std::env::set_var(DEADLINE_VARIABLE, deadline.as_secs_f64().to_string());
}

/// The termination signals: SIGINT, SIGTERM and SIGHUP; on Windows Ctrl-C,
/// Ctrl-Break and closing the console. Listened to from the start, so a
/// signal that comes while the host starts ends it once it has started.
/// On Unix, one the host was started ignoring (by `nohup`, or as a
/// script's background job) stays ignored.
pub struct Signals {
    #[cfg(unix)]
    interrupt: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    hangup: Option<tokio::signal::unix::Signal>,
    #[cfg(windows)]
    ctrl_c: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    ctrl_break: tokio::signal::windows::CtrlBreak,
    #[cfg(windows)]
    ctrl_close: tokio::signal::windows::CtrlClose,
}

impl Signals {
    pub fn listen() -> Result<Self, String> {
        let cannot = |error: std::io::Error| format!("cannot listen for signals: {error}");
        #[cfg(unix)]
        {
            use tokio::signal::unix::SignalKind;
            Ok(Self {
                interrupt: unix_signal(SignalKind::interrupt()).map_err(cannot)?,
                terminate: unix_signal(SignalKind::terminate()).map_err(cannot)?,
                hangup: unix_signal(SignalKind::hangup()).map_err(cannot)?,
            })
        }
        #[cfg(windows)]
        {
            use tokio::signal::windows;
            Ok(Self {
                ctrl_c: windows::ctrl_c().map_err(cannot)?,
                ctrl_break: windows::ctrl_break().map_err(cannot)?,
                ctrl_close: windows::ctrl_close().map_err(cannot)?,
            })
        }
    }

    /// The next signal's name.
    pub async fn next(&mut self) -> &'static str {
        #[cfg(unix)]
        {
            tokio::select! {
                () = recv(&mut self.interrupt) => "SIGINT",
                () = recv(&mut self.terminate) => "SIGTERM",
                () = recv(&mut self.hangup) => "SIGHUP",
            }
        }
        #[cfg(windows)]
        {
            tokio::select! {
                _ = self.ctrl_c.recv() => "Ctrl-C",
                _ = self.ctrl_break.recv() => "Ctrl-Break",
                _ = self.ctrl_close.recv() => "console close",
            }
        }
    }
}

/// Listen for `kind`, unless this process was started ignoring it.
#[cfg(unix)]
pub(crate) fn unix_signal(
    kind: tokio::signal::unix::SignalKind,
) -> std::io::Result<Option<tokio::signal::unix::Signal>> {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: sigaction(2) only reads the current action into `action`.
    let read = unsafe { libc::sigaction(kind.as_raw_value(), std::ptr::null(), &mut action) };
    if read == 0 && action.sa_sigaction == libc::SIG_IGN {
        return Ok(None);
    }
    tokio::signal::unix::signal(kind).map(Some)
}

/// The next of `signal`; never, when it is not listened to.
#[cfg(unix)]
pub(crate) async fn recv(signal: &mut Option<tokio::signal::unix::Signal>) {
    match signal {
        Some(signal) => {
            signal.recv().await;
        }
        None => std::future::pending().await,
    }
}

/// After `ended`: shut the host down, waiting up to `deadline` for the
/// cleanups, or until a signal ([`stop`]).
pub async fn end(host: &Host, ended: Ended, deadline: Duration, signals: &mut Signals) -> Stopped {
    match ended {
        Ended::Signal(signal) => stop(host, signal, deadline, signals).await,
        Ended::Runtime(name, why) => {
            errln!(
                "rutis-host: runtime {name} ended ({}), and its restart is \"exit\": stopping; \
                 cleanups have {} (a signal to exit at once)",
                rutis_host::status::how_it_ended(&why),
                seconds(deadline)
            );
            match finish(host, deadline, signals).await {
                Stopped::Done => Stopped::RuntimeEnded,
                other => other,
            }
        }
    }
}

/// After `signal`: shut the host down, waiting up to `deadline` for the
/// cleanups, or until another signal.
pub async fn stop(host: &Host, signal: &str, deadline: Duration, signals: &mut Signals) -> Stopped {
    errln!(
        "rutis-host: {signal}: stopping; cleanups have {} (again to exit at once)",
        seconds(deadline)
    );
    finish(host, deadline, signals).await
}

/// Shut the host down: wait up to `deadline` for the cleanups, then for
/// the runtime processes to exit (what is left of it, and at least
/// [`PROCESS_EXIT`]). A signal ends either wait at once; then, or past
/// the deadline, what is still stopping is listed and the runtime
/// processes are killed.
pub async fn finish(host: &Host, deadline: Duration, signals: &mut Signals) -> Stopped {
    let started = std::time::Instant::now();
    let ended = async {
        tokio::select! {
            result = host.shut_down(deadline) => result?,
            signal = signals.next() => return Err(Unfinished::Cleanups(signal.into())),
        }
        let left = deadline.saturating_sub(started.elapsed()).max(PROCESS_EXIT);
        tokio::select! {
            result = host.processes_exit(left) => result,
            signal = signals.next() => Err(Unfinished::Processes(signal.into())),
        }
    }
    .await;
    let Err(why) = ended else {
        return Stopped::Done;
    };
    let (stopping, waiting) = host.still_running();
    errln!("rutis-host: {why}");
    for plugin in stopping {
        errln!("  still stopping: {plugin}");
    }
    if waiting > 0 {
        errln!("  {}", waiting_line(waiting));
    }
    // What a cleanup that does not finish is often waiting on.
    for call in host.loader.status().calls {
        errln!("  call not returned: {call}");
    }
    let processes = rutis_bridge::transport::local::running_processes();
    if !processes.is_empty() {
        errln!("  killing runtime processes {processes:?}");
    }
    host.kill_processes().await;
    Stopped::Unfinished
}

/// `and 1 row waiting for them` / `and 5 rows waiting for them`.
fn waiting_line(rows: usize) -> String {
    match rows {
        1 => "and 1 row waiting for them".to_owned(),
        n => format!("and {n} rows waiting for them"),
    }
}

fn seconds(duration: Duration) -> String {
    format!("{}s", duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows still waiting are counted as rows, in the right number
    /// (#680).
    #[test]
    fn the_rows_waiting_are_counted_as_rows() {
        assert_eq!(waiting_line(1), "and 1 row waiting for them");
        assert_eq!(waiting_line(5), "and 5 rows waiting for them");
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    /// The flag, in either form, wins over the variable and leaves the
    /// other arguments; without either, the default.
    /// guarantee: host/stop-deadline
    #[test]
    fn the_deadline_comes_from_the_flag_then_the_variable() {
        let some = |value: &str| Some(value.to_owned());
        assert_eq!(
            deadline_in(&args(&["--shutdown-timeout", "2.5", "a.json"]), some("7")),
            Ok((Duration::from_millis(2500), args(&["a.json"])))
        );
        assert_eq!(
            deadline_in(&args(&["a.json", "--shutdown-timeout=0.5"]), None),
            Ok((Duration::from_millis(500), args(&["a.json"])))
        );
        assert_eq!(
            deadline_in(&args(&["a.json"]), some("7")),
            Ok((Duration::from_secs(7), args(&["a.json"])))
        );
        assert_eq!(deadline_in(&[], None), Ok((DEFAULT_DEADLINE, Vec::new())));
    }

    /// guarantee: host/stop-deadline
    #[test]
    fn a_deadline_that_is_not_a_number_of_seconds_is_an_error() {
        for bad in ["abc", "-1", "0", "inf", ""] {
            let error = deadline_in(&args(&["--shutdown-timeout", bad]), None).unwrap_err();
            assert!(
                error.contains("is not a number of seconds"),
                "{bad}: {error}"
            );
        }
        let error = deadline_in(&[], Some("soon".into())).unwrap_err();
        assert!(error.starts_with(DEADLINE_VARIABLE), "{error}");
        let error = deadline_in(&args(&["--shutdown-timeout"]), None).unwrap_err();
        assert!(error.contains("needs a number"), "{error}");
    }
}

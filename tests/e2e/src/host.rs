//! A `rutis-host` process: started with its scenario's marker in its
//! environment, in its own process group (Unix) or Job Object (Windows),
//! its stdout and stderr captured line by line, waited on with a hang
//! guard.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::hang_guard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// A line a host (or a process it started, which shares its output) wrote.
#[derive(Debug, Clone)]
pub struct Line {
    pub stream: Stream,
    pub text: String,
}

/// Everything a host wrote, as it arrives.
#[derive(Default)]
pub(crate) struct Output {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    lines: Vec<Line>,
    /// Lines a wait already matched; each line matches one wait.
    taken: Vec<bool>,
    /// Streams at their end.
    closed: usize,
}

impl Output {
    fn read(self: Arc<Self>, stream: Stream, source: impl Read + Send + 'static) {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(source);
            let mut buffer = Vec::new();
            loop {
                buffer.clear();
                match reader.read_until(b'\n', &mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&buffer)
                            .trim_end_matches(['\n', '\r'])
                            .to_owned();
                        let mut state = self.state.lock().unwrap();
                        state.lines.push(Line { stream, text });
                        state.taken.push(false);
                        self.changed.notify_all();
                    }
                }
            }
            self.state.lock().unwrap().closed += 1;
            self.changed.notify_all();
        });
    }

    pub(crate) fn lines(&self) -> Vec<Line> {
        self.state.lock().unwrap().lines.clone()
    }

    /// The output as a log file: `out| …` and `err| …` lines in order.
    pub(crate) fn transcript(&self) -> String {
        self.lines()
            .iter()
            .map(|line| match line.stream {
                Stream::Stdout => format!("out| {}\n", line.text),
                Stream::Stderr => format!("err| {}\n", line.text),
            })
            .collect()
    }
}

/// What the scenario keeps of each host it started, for the residue checks
/// and the failure logs.
pub(crate) struct Started {
    pub name: String,
    pub pid: u32,
    pub output: Arc<Output>,
    /// The job it and everything it starts are in.
    #[cfg(windows)]
    pub job: Arc<crate::job::Job>,
}

/// A running `rutis-host`. Dropping it while it runs kills it with its
/// process group (Unix) or its whole job (Windows), so a failing scenario
/// leaves nothing behind.
pub struct Host {
    name: String,
    #[cfg_attr(not(unix), allow(dead_code))]
    marker: String,
    child: Child,
    /// Its stdin, when the scenario writes to it ([`Host::send_line`]).
    stdin: Option<ChildStdin>,
    output: Arc<Output>,
    status: Option<ExitStatus>,
    #[cfg(windows)]
    job: Arc<crate::job::Job>,
}

impl Host {
    /// Start `command` with [`MARKER`](crate::MARKER) `marker`, which every
    /// process it starts inherits: the residue checks find them by it.
    /// With `input`, its stdin is a pipe for [`Host::send_line`], else
    /// empty.
    pub(crate) fn spawn(
        name: &str,
        marker: &str,
        mut command: Command,
        input: bool,
    ) -> (Host, Started) {
        command
            .env(crate::MARKER, marker)
            .stdin(if input { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own group, which a terminal's Ctrl-C would signal
        // ([`Host::ctrl_c`]). Only the host's: rutis-host puts each runtime
        // in a group of its own.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        // Its own console process group (the target of
        // [`Host::ctrl_c`]), and suspended until it is in its job.
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(
            &mut command,
            windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP
                | windows_sys::Win32::System::Threading::CREATE_SUSPENDED,
        );
        #[cfg(windows)]
        let job = Arc::new(
            crate::job::Job::new().unwrap_or_else(|error| panic!("cannot create a job: {error}")),
        );
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("cannot start {:?}: {error}", command.get_program()));
        crate::residue::started(child.id());
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            let placed = job
                .assign(child.as_raw_handle())
                .and_then(|()| crate::job::resume(child.id()));
            if let Err(error) = placed {
                let _ = child.kill();
                let _ = child.wait();
                panic!("cannot start {:?} in a job: {error}", command.get_program());
            }
        }
        let output = Arc::new(Output::default());
        output
            .clone()
            .read(Stream::Stdout, child.stdout.take().unwrap());
        output
            .clone()
            .read(Stream::Stderr, child.stderr.take().unwrap());
        let started = Started {
            name: name.to_owned(),
            pid: child.id(),
            output: output.clone(),
            #[cfg(windows)]
            job: job.clone(),
        };
        let stdin = child.stdin.take();
        let host = Host {
            name: name.to_owned(),
            marker: marker.to_owned(),
            child,
            stdin,
            output,
            status: None,
            #[cfg(windows)]
            job,
        };
        (host, started)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Its value of [`MARKER`](crate::MARKER).
    #[cfg(unix)]
    pub(crate) fn marker(&self) -> &str {
        &self.marker
    }

    #[cfg(windows)]
    pub(crate) fn job(&self) -> &crate::job::Job {
        &self.job
    }

    /// Everything it wrote so far, stdout and stderr in arrival order.
    pub fn lines(&self) -> Vec<Line> {
        self.output.lines()
    }

    /// Count every line written so far as matched: later waits see only
    /// what comes after. For a wait on a line the host printed before too
    /// (a row's state it returns to, say), once what came before is
    /// settled.
    pub fn skip_written(&mut self) {
        let mut state = self.output.state.lock().unwrap();
        state.taken.iter_mut().for_each(|taken| *taken = true);
    }

    /// Wait for a line containing `text` that no earlier wait matched.
    pub fn expect(&mut self, text: &str) -> String {
        self.wait_for(&format!("a line containing {text:?}"), |line| {
            line.contains(text)
        })
    }

    /// Wait for a line (stdout or stderr) that `matches` and that no
    /// earlier wait matched, and return it. Lines written before the call
    /// count: what happened before the wait is not lost. Fails, with the
    /// output so far, when the output ends or the hang guard expires
    /// first; `what` names the wait in that message.
    pub fn wait_for(&mut self, what: &str, matches: impl Fn(&str) -> bool) -> String {
        self.wait_for_within(hang_guard(), what, matches)
    }

    /// [`Host::wait_for`], waiting up to `guard` instead of the hang guard:
    /// for a step whose length is its work's (a build), not a hang's.
    pub fn wait_for_within(
        &mut self,
        guard: Duration,
        what: &str,
        matches: impl Fn(&str) -> bool,
    ) -> String {
        let deadline = Instant::now() + guard;
        let mut state = self.output.state.lock().unwrap();
        loop {
            // A probe's line with something else inside it, or inside
            // something else, is a fault of the host's, not a line to skip
            // until the hang guard (#332).
            if let Some(line) = state.lines.iter().find(|line| broken(&line.text)) {
                let text = line.text.clone();
                drop(state);
                self.fail(&format!(
                    "a probe's line came out broken (#332), waiting for {what}: {text:?}"
                ));
            }
            let found = (0..state.lines.len())
                .find(|&index| !state.taken[index] && matches(&state.lines[index].text));
            if let Some(index) = found {
                state.taken[index] = true;
                return state.lines[index].text.clone();
            }
            if state.closed == 2 {
                drop(state);
                self.fail(&format!("its output ended before {what}"));
            }
            let now = Instant::now();
            if now >= deadline {
                drop(state);
                self.fail(&format!("no {what} within {guard:?}"));
            }
            state = self
                .output
                .changed
                .wait_timeout(state, deadline - now)
                .unwrap()
                .0;
        }
    }

    /// Send `signal` to the host process alone (Unix).
    #[cfg(unix)]
    pub fn signal(&self, signal: i32) {
        // SAFETY: kill(2) on a pid this scenario started and has not reaped.
        if unsafe { libc::kill(self.pid() as i32, signal) } != 0 {
            let error = std::io::Error::last_os_error();
            self.fail(&format!("kill({signal}) failed: {error}"));
        }
    }

    /// Send `signal` to the host's process group, as a terminal does for
    /// Ctrl-C (Unix).
    #[cfg(unix)]
    pub fn signal_group(&self, signal: i32) {
        // SAFETY: killpg(2) on the group this scenario made for the host.
        if unsafe { libc::killpg(self.pid() as i32, signal) } != 0 {
            let error = std::io::Error::last_os_error();
            self.fail(&format!("killpg({signal}) failed: {error}"));
        }
    }

    /// Ctrl-C as a terminal sends it: SIGINT to the host's process group
    /// (Unix). On Windows, Ctrl-Break to the host's console process group:
    /// a process started in a new group has Ctrl-C turned off, Ctrl-Break
    /// it still gets, and rutis-host stops on either. It needs the console
    /// this test process runs in, which the host shares.
    pub fn ctrl_c(&self) {
        if self.status.is_some() {
            self.fail("Ctrl-C to a host that has exited");
        }
        #[cfg(unix)]
        self.signal_group(libc::SIGINT);
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
            use windows_sys::Win32::System::Console::{GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT};
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            // Only to a host still running: a group id that no longer
            // exists may be taken as 0, every process on the console (this
            // test run with its other scenarios). The harness holds the
            // host's handle, so its id is not reused meanwhile.
            // SAFETY: the handle is live while `child` is owned.
            let running =
                unsafe { WaitForSingleObject(self.child.as_raw_handle() as _, 0) } == WAIT_TIMEOUT;
            if !running {
                self.fail("Ctrl-C to a host that has exited");
            }
            // SAFETY: a plain Win32 call; the group is the host's, made for
            // it when it was started, and it still runs.
            if unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, self.pid()) } == 0 {
                let error = std::io::Error::last_os_error();
                self.fail(&format!("Ctrl-Break failed: {error}"));
            }
        }
    }

    /// Write `line` and a newline to its stdin: a command to a host that
    /// reads them (the fixture host,
    /// [`Scenario::fixture_host`](crate::Scenario::fixture_host)).
    pub fn send_line(&mut self, line: &str) {
        let Some(stdin) = self.stdin.as_mut() else {
            self.fail("its stdin is not open for commands");
        };
        let written = stdin
            .write_all(format!("{line}\n").as_bytes())
            .and_then(|()| stdin.flush());
        if let Err(error) = written {
            self.fail(&format!("cannot write {line:?} to its stdin: {error}"));
        }
    }

    /// How many lines so far contain `text`. To count "exactly once", wait
    /// for the output to end first ([`Host::wait_output_end`]).
    pub fn count(&self, text: &str) -> usize {
        self.output
            .lines()
            .iter()
            .filter(|line| line.text.contains(text))
            .count()
    }

    /// Kill the host process alone (SIGKILL; TerminateProcess on Windows):
    /// what it started has to end by itself.
    pub fn kill(&mut self) {
        if self.status.is_none() {
            if let Err(error) = self.child.kill() {
                self.fail(&format!("kill failed: {error}"));
            }
        }
    }

    /// Wait until the host's output has ended (stdout and stderr closed:
    /// it and everything it started that shares them are gone), within the
    /// hang guard. Then [`Host::count`] counts everything it wrote.
    pub fn wait_output_end(&mut self) {
        let deadline = Instant::now() + hang_guard();
        let mut state = self.output.state.lock().unwrap();
        while state.closed < 2 {
            let now = Instant::now();
            if now >= deadline {
                drop(state);
                self.fail(&format!("its output did not end within {:?}", hang_guard()));
            }
            state = self
                .output
                .changed
                .wait_timeout(state, deadline - now)
                .unwrap()
                .0;
        }
    }

    /// Wait until the host process exits, within the hang guard.
    pub fn wait_exit(&mut self) -> ExitStatus {
        self.wait_exit_within(hang_guard())
    }

    /// Wait until the process exits, within `guard`: [`crate::build_guard`]
    /// for a build or an install.
    pub fn wait_exit_within(&mut self, guard: Duration) -> ExitStatus {
        if let Some(status) = self.status {
            return status;
        }
        let deadline = Instant::now() + guard;
        loop {
            if let Some(status) = self.child.try_wait().expect("the host's status") {
                self.status = Some(status);
                return status;
            }
            if Instant::now() >= deadline {
                self.fail(&format!("it did not exit within {guard:?}"));
            }
            // Polling the exit: std has no wait with a timeout.
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn fail(&self, reason: &str) -> ! {
        let lines = self.output.lines();
        let tail: Vec<&str> = lines
            .iter()
            .rev()
            .take(40)
            .rev()
            .map(|line| line.text.as_str())
            .collect();
        panic!(
            "{} (pid {}): {reason}\nlast output:\n  {}",
            self.name,
            self.pid(),
            tail.join("\n  ")
        );
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if self.status.is_none() {
            // SAFETY: killpg(2) on the group this scenario made for the host.
            #[cfg(unix)]
            unsafe {
                libc::killpg(self.pid() as i32, libc::SIGKILL);
            }
            #[cfg(windows)]
            self.job.terminate();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Whether `text` has a probe's line (`{"probe": …}`, one JSON object per
/// line, maybe after its row's name) in it without being one whole: lines
/// ran into each other.
fn broken(text: &str) -> bool {
    let text = crate::unprefixed(text);
    text.contains("{\"probe\"")
        && !(text.starts_with("{\"probe\"")
            && serde_json::from_str::<serde_json::Value>(text).is_ok())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_probe_line_run_into_another_is_broken() {
        assert!(!super::broken(r#"{"probe": "p", "event": "started"}"#));
        assert!(!super::broken("demo: running"));
        assert!(!super::broken(
            r#"[probe] {"probe": "p", "event": "started"}"#
        ));
        assert!(super::broken(
            r#"{"probe": "p", "event": "started"}demo: running"#
        ));
        assert!(super::broken(r#"demo: {"probe": "p", "event": "started"}"#));
        assert!(super::broken(
            r#"{"probe":"a","event":"started"}{"probe":"b","event":"started"}"#
        ));
    }
}

//! A `rutis-host` process: started in its own process group (Unix), its
//! stdout and stderr captured line by line, waited on with a hang guard.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
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
}

/// A running `rutis-host`. Dropping it kills its process group (Unix), so
/// a failing scenario leaves nothing behind; on Windows only the host
/// itself, its runtimes ending with its job (a Job Object of the harness's
/// own is #232).
pub struct Host {
    name: String,
    child: Child,
    output: Arc<Output>,
    status: Option<ExitStatus>,
}

impl Host {
    pub(crate) fn spawn(name: &str, mut command: Command) -> (Host, Started) {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own group: what it starts can be found, and signalled, as a
        // group (the runtimes inherit it).
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("cannot start {:?}: {error}", command.get_program()));
        crate::residue::started(child.id());
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
        };
        let host = Host {
            name: name.to_owned(),
            child,
            output,
            status: None,
        };
        (host, started)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Everything it wrote so far, stdout and stderr in arrival order.
    pub fn lines(&self) -> Vec<Line> {
        self.output.lines()
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
        let deadline = Instant::now() + hang_guard();
        let mut state = self.output.state.lock().unwrap();
        loop {
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
                self.fail(&format!("no {what} within {:?} (hang guard)", hang_guard()));
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
    /// (Unix).
    #[cfg(unix)]
    pub fn ctrl_c(&self) {
        self.signal_group(libc::SIGINT);
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
        if let Some(status) = self.status {
            return status;
        }
        let deadline = Instant::now() + hang_guard();
        loop {
            if let Some(status) = self.child.try_wait().expect("the host's status") {
                self.status = Some(status);
                return status;
            }
            if Instant::now() >= deadline {
                self.fail(&format!("it did not exit within {:?}", hang_guard()));
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
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

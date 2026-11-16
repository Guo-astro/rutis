//! Shuttle models of the subprocess exit state (#623, merged into #622): how
//! a channel that owns a process ends, and what it says.
//!
//! "The channel's end says how the process ended" was fixed four times, each
//! a race between the reader thread, the sending thread (holding the
//! session's writer), the closing side and the task that waits for the
//! process: #157 (a runtime that exited early made the first send report
//! Broken pipe), #243 (EOF then a one-second wait for the status), #246 and
//! #261b (statuses lost under load), #252 (a failed send set the reason
//! before the status). The models run the production `Exit`, `Ended`,
//! `EndedSender`, `Owning::close` and `Child::end`: `crate::sync` makes the
//! exit state's mutex, condition variable and `closed` flag shuttle's, so
//! every access is a point where the scheduler may switch threads, and
//! `shuttle::thread` stands in for the reader, the sender, the closer and
//! the task that waits for the process.
//!
//! What the models stand in for, and why: the process itself (`Child::watch`
//! starts one and waits on tokio, which no model drives), and the two
//! real-time guards — `GRACE` (how long a process whose channel closed here
//! may take to end) and `HANG_GUARD` (how long the channel's end waits for
//! the status). Real time is not an interleaving: under `--cfg rutis_shuttle`
//! the swappable condition variable never times out, so a wait has to be
//! woken by the model's own `record`/`closed_here` step, and a thread nobody
//! wakes fails the iteration as a deadlock. The real process path is pinned
//! end to end by `tests/process_exit.rs` (a runtime that exits with 17);
//! `spawn.rs`'s unit tests pin what a wait must see before it.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-bridge --lib \
//!     --target-dir target/shuttle shuttle
//! ```
//!
//! `RUTIS_SHUTTLE_ITERATIONS` sets the iterations of each scheduler: 1000
//! by default, as on pull requests; the nightly run (`stress.yml`) uses a
//! million. A failure prints its schedule; `RUTIS_SHUTTLE_REPLAY=<schedule>`
//! replays it, with the failing test's name as the filter.
//!
//! guarantee: bridge/in-flight-call-fails-once

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use shuttle::future::block_on;
use shuttle::sync::{Barrier, Mutex};
use shuttle::thread;

use super::{Child, Ended, EndedSender, Exit, Owning, Processes, HANG_GUARD};
use crate::channel::{ChannelError, Close, Closer, Receiver, Sender};

/// What the process's end says (`describe`, for a process that exited by
/// itself and for one a signal ended).
const EXIT_STATUS: &str = "exited with exit status: 17";
const SIGNAL: &str = "exited with signal: 9 (SIGKILL)";

/// Iterations of each scheduler: `RUTIS_SHUTTLE_ITERATIONS`, or 1000.
fn iterations() -> usize {
    std::env::var("RUTIS_SHUTTLE_ITERATIONS")
        .ok()
        .map(|value| value.parse().expect("RUTIS_SHUTTLE_ITERATIONS is a number"))
        .unwrap_or(1000)
}

/// Explore `model` with shuttle's random scheduler and with PCT (which
/// finds bugs that need a few preemptions at the right places with better
/// odds than uniform choices), or replay `RUTIS_SHUTTLE_REPLAY`.
fn explore<F>(model: F)
where
    F: Fn() + Send + Sync + 'static,
{
    if let Ok(schedule) = std::env::var("RUTIS_SHUTTLE_REPLAY") {
        shuttle::replay(model, schedule.trim().trim_matches('"'));
        return;
    }
    let model = Arc::new(model);
    let iterations = iterations();
    let random = model.clone();
    shuttle::check_random(move || random(), iterations);
    shuttle::check_pct(move || model(), iterations, 3);
}

/// A receiver at EOF: the process closed its channel.
struct Ends;
impl Receiver for Ends {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
        Ok(None)
    }
}

/// A sender the far end closed on: the process exited before reading.
struct Broken;
impl Sender for Broken {
    fn send(&mut self, _: &[u8]) -> Result<(), ChannelError> {
        Err(ChannelError::Closed {
            reason: "Broken pipe".into(),
        })
    }
}

/// The channel's own closer, counting: `Owning` closes it after it marks
/// the close here, and before it asks the process to end.
#[derive(Default)]
struct Counts(AtomicUsize);

impl Counts {
    fn closes(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

impl Closer for Counts {
    fn close(&self, _why: Close) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// The channel's owner around `exit`, and the process's end signal: what
/// `Owning::close` ends, without a process to start.
fn owning(
    exit: &Arc<Exit>,
    closer: Arc<Counts>,
) -> (Arc<Owning>, tokio::sync::oneshot::Receiver<()>) {
    let (kill, killed) = tokio::sync::oneshot::channel();
    (
        Arc::new(Owning {
            closer,
            child: Child {
                ended: exit.clone(),
                end: StdMutex::new(Some(kill)),
            },
            _directory: None,
        }),
        killed,
    )
}

/// #246, #252: the channel's end waits for the process, even when this side
/// closed the channel first: it says how the process ended, not that the
/// channel closed or ended by itself. The read is the reader thread's.
#[test]
fn the_end_of_the_channel_says_how_the_process_ended() {
    explore(|| {
        let exit = Arc::new(Exit::default());
        let (owning, mut killed) = owning(&exit, Arc::new(Counts::default()));
        let mut reader = Ended {
            receiver: Box::new(Ends),
            ended: Some(exit.clone()),
        };

        let closing = {
            let owning = owning.clone();
            thread::spawn(move || owning.close(Close::Done))
        };
        let recording = {
            let exit = exit.clone();
            thread::spawn(move || exit.record(EXIT_STATUS.to_owned()))
        };

        let read = reader.recv();
        closing.join().unwrap();
        recording.join().unwrap();

        match read {
            Err(ChannelError::Closed { reason }) => {
                assert_eq!(reason, format!("the process {EXIT_STATUS}"))
            }
            other => panic!("the channel ended as {other:?}, not with how the process ended"),
        }
        assert_eq!(
            killed.try_recv(),
            Ok(()),
            "closing the channel did not ask the process to end"
        );
    });
}

/// #157, #252: a process that exited before reading fails the first send;
/// the send says how the process ended, not Broken pipe, whichever of the
/// two runs first.
#[test]
fn a_failed_send_says_how_the_process_ended() {
    explore(|| {
        let exit = Arc::new(Exit::default());
        let mut sender = EndedSender {
            sender: Box::new(Broken),
            ended: exit.clone(),
        };

        let recording = {
            let exit = exit.clone();
            thread::spawn(move || exit.record(SIGNAL.to_owned()))
        };

        let sent = sender.send(b"{}");
        recording.join().unwrap();

        match sent {
            Err(ChannelError::Closed { reason }) => {
                assert_eq!(reason, format!("the process {SIGNAL}"))
            }
            other => panic!("the send ended as {other:?}, not with how the process ended"),
        }
    });
}

/// #252: a send blocked on the process holds the session's writer, which
/// closing the session needs. Closing here wakes the send without that
/// writer, and without waiting for the process: the send fails at once with
/// its own error, and the close goes on to end the process.
#[test]
fn a_send_waiting_for_the_process_is_woken_by_closing_here() {
    explore(|| {
        let exit = Arc::new(Exit::default());
        let counts = Arc::new(Counts::default());
        let (owning, mut killed) = owning(&exit, counts.clone());
        // The session's writer: a send holds it, and closing the session
        // needs it.
        let writer = Arc::new(Mutex::new(()));
        let started = Arc::new(Barrier::new(2));
        let mut sender = EndedSender {
            sender: Box::new(Broken),
            ended: exit.clone(),
        };

        let sending = {
            let writer = writer.clone();
            let started = started.clone();
            thread::spawn(move || {
                let writer = writer.lock().unwrap();
                started.wait();
                let sent = sender.send(b"{}");
                drop(writer);
                sent
            })
        };
        let closing = {
            let writer = writer.clone();
            let started = started.clone();
            thread::spawn(move || {
                started.wait();
                owning.close(Close::Done);
                // Taken once the send let go: the close waited neither for
                // the process nor for the send's wait.
                let _ = writer.lock().unwrap();
            })
        };

        match sending.join().unwrap() {
            Err(ChannelError::Closed { reason }) => assert_eq!(reason, "Broken pipe"),
            other => panic!("the send ended as {other:?}, not with its own error"),
        }
        closing.join().unwrap();
        assert_eq!(counts.closes(), 1, "the channel was not closed once");
        assert_eq!(
            killed.try_recv(),
            Ok(()),
            "the process was not asked to end"
        );
    });
}

/// A message the framing refuses fails without waiting for the process,
/// even while the channel is closed here and the process ends: the send
/// decides before either, so neither the close nor the status changes its
/// error.
#[test]
fn a_refused_message_does_not_wait_for_the_process() {
    explore(|| {
        let exit = Arc::new(Exit::default());
        let mut sender = EndedSender {
            sender: Box::new(Broken),
            ended: exit.clone(),
        };
        // The send finishes before these two are let through: a send that
        // waited here would leave the model deadlocked at this barrier.
        let step = Arc::new(Barrier::new(3));
        let recording = {
            let exit = exit.clone();
            let step = step.clone();
            thread::spawn(move || {
                step.wait();
                exit.record(EXIT_STATUS.to_owned());
            })
        };
        let closing = {
            let exit = exit.clone();
            let step = step.clone();
            thread::spawn(move || {
                step.wait();
                exit.closed_here();
            })
        };

        let sent = sender.send(b"a\nb");
        assert!(
            exit.0.lock().unwrap().is_none(),
            "the refused send waited for the process"
        );
        step.wait();
        recording.join().unwrap();
        closing.join().unwrap();

        match sent {
            Err(ChannelError::Closed { reason }) => assert_eq!(reason, "Broken pipe"),
            other => panic!("the refused send ended as {other:?}, not with its own error"),
        }
    });
}

/// A record before the wait is not lost: the wait wakes with the status,
/// whether the record ran before or after the wait began.
#[test]
fn a_record_before_the_wait_is_not_lost() {
    explore(|| {
        let exit = Arc::new(Exit::default());
        let started = Arc::new(Barrier::new(2));
        let waiting = {
            let exit = exit.clone();
            let started = started.clone();
            thread::spawn(move || {
                started.wait();
                exit.wait(HANG_GUARD)
            })
        };
        let recording = {
            let exit = exit.clone();
            let started = started.clone();
            thread::spawn(move || {
                started.wait();
                exit.record(EXIT_STATUS.to_owned())
            })
        };

        let got = waiting.join().unwrap();
        recording.join().unwrap();
        assert_eq!(got.as_deref(), Some(EXIT_STATUS));
    });
}

/// A close here before the send waits is not lost: the send wakes at once
/// with its own error, whether the close ran before or after the wait
/// began.
#[test]
fn a_close_before_the_wait_is_not_lost() {
    explore(|| {
        let exit = Arc::new(Exit::default());
        let started = Arc::new(Barrier::new(2));
        let waiting = {
            let exit = exit.clone();
            let started = started.clone();
            thread::spawn(move || {
                started.wait();
                exit.wait_unless_closed(HANG_GUARD)
            })
        };
        let closing = {
            let exit = exit.clone();
            let started = started.clone();
            thread::spawn(move || {
                started.wait();
                exit.closed_here();
            })
        };

        let got = waiting.join().unwrap();
        closing.join().unwrap();
        assert_eq!(got, None);
    });
}

/// A record and a close racing one send's wait: the wait always ends (the
/// process ending and this side closing both wake it), and the status is
/// recorded whether or not the wait saw it.
#[test]
fn a_record_and_a_close_racing_a_send_do_not_lose_the_status() {
    explore(|| {
        let exit = Arc::new(Exit::default());
        let started = Arc::new(Barrier::new(3));
        let waiting = {
            let exit = exit.clone();
            let started = started.clone();
            thread::spawn(move || {
                started.wait();
                exit.wait_unless_closed(HANG_GUARD)
            })
        };
        let recording = {
            let exit = exit.clone();
            let started = started.clone();
            thread::spawn(move || {
                started.wait();
                exit.record(SIGNAL.to_owned())
            })
        };
        let closing = {
            let exit = exit.clone();
            let started = started.clone();
            thread::spawn(move || {
                started.wait();
                exit.closed_here();
            })
        };

        let got = waiting.join().unwrap();
        recording.join().unwrap();
        closing.join().unwrap();
        assert!(
            got.is_none() || got.as_deref() == Some(SIGNAL),
            "the wait returned {got:?}, which is neither the close here nor the status"
        );
        assert_eq!(exit.0.lock().unwrap().as_deref(), Some(SIGNAL));
    });
}

/// A kill (`notify_one`'s one permit) and the task that waits for the
/// process: asked before the task starts it is kept, and asked after the
/// task waits it wakes it. Either way the task goes on to kill the process,
/// which `Processes::kill` relies on when a runtime did not stop in its
/// deadline.
#[test]
fn a_kill_asked_before_the_task_started_is_kept() {
    explore(|| {
        let exit = Arc::new(Exit::default());
        let processes = Processes(Arc::new(StdMutex::new(vec![exit.clone()])));
        let started = Arc::new(Barrier::new(2));
        let waiting = {
            let exit = exit.clone();
            let started = started.clone();
            thread::spawn(move || {
                started.wait();
                block_on(exit.4.notified())
            })
        };
        let killing = {
            let started = started.clone();
            thread::spawn(move || {
                started.wait();
                processes.kill();
            })
        };
        waiting.join().unwrap();
        killing.join().unwrap();
    });
}

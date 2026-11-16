//! Shuttle model of the line framing's writer and the handle it writes with
//! (#628): a close racing a send.
//!
//! `Writer::release` is called by whoever closes the channel, on a thread of
//! its own, while a send may be holding the handle with the lock: a write the
//! far end stopped reading need not wake when the stream is shut down
//! (Winsock), and the thread that closes may be what ends the session. What
//! the model checks: the release never waits for the send, a send that
//! already held the handle finishes and lets go of it, and the handle is
//! dropped exactly once.
//!
//! The model runs the production `LineSender::send`, `Writer::release`,
//! `Writer::released_meanwhile` and `Releasing::close`. `crate::sync` makes
//! the writer's handle and its `released` flag shuttle's under
//! `--cfg rutis_shuttle`, so a release and a send interleave at both.
//!
//! Run (CONTRIBUTING.md):
//!
//! ```text
//! RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis-bridge --lib \
//!     --target-dir target/shuttle shuttle
//! ```

use std::io::{self, IoSlice, Write};
use std::sync::Arc;

use shuttle::sync::Barrier;
use shuttle::thread;

use super::{End, LineSender, Releasing, Writer, MAX_MESSAGE};
use crate::channel::{Close, Closer, Sender};
use crate::sync::Mutex as ModelMutex;

/// Iterations of each scheduler: `RUTIS_SHUTTLE_ITERATIONS`, or 1000.
fn iterations() -> usize {
    std::env::var("RUTIS_SHUTTLE_ITERATIONS")
        .ok()
        .map(|value| value.parse().expect("RUTIS_SHUTTLE_ITERATIONS is a number"))
        .unwrap_or(1000)
}

/// Explore `model` with shuttle's random scheduler and with PCT.
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

/// A closer that closes nothing.
struct Nothing;

impl Closer for Nothing {
    fn close(&self, _: Close) {}
}

/// The streams the model wrote with, dropped: one entry per handle dropped.
#[derive(Default)]
struct Dropped(ModelMutex<Vec<u64>>);

impl Dropped {
    fn ids(&self) -> Vec<u64> {
        self.0.lock().unwrap().clone()
    }
}

/// A stream of the model: it counts its drop and, for a send that is meant
/// to hold the handle, waits at `writing` for the closer and then at `go`
/// for it to let go.
struct Stream {
    id: u64,
    dropped: Arc<Dropped>,
    writing: Option<Arc<Barrier>>,
    go: Option<Arc<Barrier>>,
    written: Arc<ModelMutex<Vec<u8>>>,
}

impl Stream {
    fn new(id: u64, dropped: Arc<Dropped>) -> Self {
        Self {
            id,
            dropped,
            writing: None,
            go: None,
            written: Arc::default(),
        }
    }

    /// A stream whose write holds the handle until the closer reached `go`.
    fn blocked(id: u64, dropped: Arc<Dropped>, writing: Arc<Barrier>, go: Arc<Barrier>) -> Self {
        Self {
            id,
            dropped,
            writing: Some(writing),
            go: Some(go),
            written: Arc::default(),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.written.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn write_vectored(&mut self, parts: &[IoSlice<'_>]) -> io::Result<usize> {
        if let (Some(writing), Some(go)) = (&self.writing, &self.go) {
            // The send now holds the handle.
            writing.wait();
            // The closer let go of it and returned while this send still
            // held it; only then may the write go on.
            go.wait();
        }
        let mut written = 0;
        for part in parts {
            self.written.lock().unwrap().extend_from_slice(part);
            written += part.len();
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.dropped.0.lock().unwrap().push(self.id);
    }
}

/// A sender over `stream`, and the writer it writes through (which a closer
/// holds, as the channel's closer does).
fn sender(stream: Stream) -> (Arc<Writer<Stream>>, LineSender<Stream>) {
    let writer = Arc::new(Writer::new(stream));
    let sender = LineSender(
        writer.clone(),
        End {
            ended: Arc::default(),
            closer: Arc::new(Nothing),
            max_message: MAX_MESSAGE,
        },
    );
    (writer, sender)
}

/// #628: whatever order a release and a send take, the handle is written
/// with at most once and dropped exactly once, and a close never waits for
/// a send that holds it.
#[test]
fn a_release_lets_go_of_the_handle_once() {
    explore(|| {
        // Two releases, no send: the handle goes, once; a send after them
        // is refused and drops nothing more.
        let dropped = Arc::new(Dropped::default());
        let (writer, mut line) = sender(Stream::new(1, dropped.clone()));
        let step = Arc::new(Barrier::new(3));
        let mut releasing = Vec::new();
        for _ in 0..2 {
            let (writer, step) = (writer.clone(), step.clone());
            releasing.push(thread::spawn(move || {
                step.wait();
                writer.release();
            }));
        }
        step.wait();
        for release in releasing {
            release.join().unwrap();
        }
        assert_eq!(
            dropped.ids(),
            vec![1],
            "a released handle was not dropped once"
        );
        assert!(
            line.send(b"{}").is_err(),
            "a released writer took a message"
        );
        assert_eq!(
            dropped.ids(),
            vec![1],
            "a send after the release dropped the handle again"
        );

        // A send holding the handle and a release: the release returns
        // without waiting for the send; the send finishes writing, then
        // drops the handle it held — once. A deadlock here is the release
        // waiting for the send, which it must not.
        let blocked = || {
            let dropped = Arc::new(Dropped::default());
            let writing = Arc::new(Barrier::new(2));
            let go = Arc::new(Barrier::new(2));
            let stream = Stream::blocked(3, dropped.clone(), writing.clone(), go.clone());
            let (writer, line) = sender(stream);
            (dropped, writing, go, writer, line)
        };
        let (dropped, writing, go, writer, mut line) = blocked();
        // Held until the model ends: nothing else may drop the handle.
        let keep = writer.clone();
        let step = Arc::new(Barrier::new(3));
        let sending = {
            let step = step.clone();
            thread::spawn(move || {
                step.wait();
                line.send(b"{}")
            })
        };
        let releasing = {
            let step = step.clone();
            thread::spawn(move || {
                step.wait();
                writing.wait();
                writer.release();
                go.wait();
            })
        };
        step.wait();
        releasing.join().unwrap();
        let sent = sending.join().unwrap();
        assert!(sent.is_ok(), "a send that held the handle was refused");
        assert_eq!(
            dropped.ids(),
            vec![3],
            "the handle a send held was not dropped exactly once"
        );
        drop(keep);

        // The same through the channel's close: closing the stream first
        // never waits for the send that holds the handle either.
        let (dropped, writing, go, writer, mut line) = blocked();
        // Held until the model ends: nothing else may drop the handle.
        let keep = writer.clone();
        let step = Arc::new(Barrier::new(3));
        let sending = {
            let step = step.clone();
            thread::spawn(move || {
                step.wait();
                line.send(b"{}")
            })
        };
        let closing = {
            let step = step.clone();
            thread::spawn(move || {
                step.wait();
                writing.wait();
                let closer = Releasing {
                    closer: Arc::new(Nothing),
                    write: writer.clone(),
                };
                closer.close(Close::Done);
                go.wait();
            })
        };
        step.wait();
        closing.join().unwrap();
        let sent = sending.join().unwrap();
        assert!(sent.is_ok(), "a send that held the handle was refused");
        assert_eq!(
            dropped.ids(),
            vec![3],
            "the handle a close let go of was not dropped exactly once"
        );
        drop(keep);
    });
}

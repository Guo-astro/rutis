//! Newline framing for byte-stream transports (Unix sockets, inherited
//! fds): the channel appends `\n` to each message it sends and strips it
//! from each one it receives. Messages must not contain a raw newline; the
//! session codec escapes newlines inside strings and emits none between
//! tokens.

use std::io::{BufRead, BufReader, IoSlice, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};

use crate::channel::{Channel, ChannelError, ChannelInfo, Closer, Receiver, Sender};

/// The largest message either way, without its newline: 16 MiB, the
/// WebSocket transport's default. Over it, the channel closes; a far end
/// that never sends a newline costs at most this much memory.
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// Frame a byte stream as a [`Channel`]. `read` and `write` are the two
/// directions of one stream (for a socket, two handles of it); `closer`
/// must wake a thread blocked on either. Once the channel is closed, or its
/// receiver saw the end, it lets go of `write` and `read` ([`Releasing`]).
pub fn channel(
    read: impl Read + Send + 'static,
    write: impl Write + Send + 'static,
    closer: Arc<dyn Closer>,
    info: ChannelInfo,
) -> Channel {
    limited(read, write, closer, info, MAX_MESSAGE)
}

/// [`channel`] with messages of at most `max_message` bytes.
pub(crate) fn limited(
    read: impl Read + Send + 'static,
    write: impl Write + Send + 'static,
    closer: Arc<dyn Closer>,
    info: ChannelInfo,
    max_message: usize,
) -> Channel {
    let write = Arc::new(Writer::new(write));
    let closer: Arc<dyn Closer> = Arc::new(Releasing {
        closer,
        write: write.clone(),
    });
    let end = End {
        ended: Arc::new(AtomicBool::new(false)),
        closer: closer.clone(),
        max_message,
    };
    Channel {
        sender: Box::new(LineSender(write, end.clone())),
        receiver: Box::new(LineReceiver(Some(BufReader::new(read)), end)),
        closer,
        info,
    }
}

/// Closes the stream, then lets go of the handle this side writes with;
/// the receiver lets go of its own once its thread sees the end, and the
/// closer of a socket of its own as it closes. The far end learns that the
/// channel ended from the stream's end, and a shutdown does not always tell
/// it: on macOS, a Unix socket shut down while its far end takes in what
/// was just sent to it, and a thread here waits to read, can be left
/// disconnected without the far end ever seeing its end (a runtime whose
/// host closed the session ran on). Closing its last handle always tells
/// it.
struct Releasing<W> {
    closer: Arc<dyn Closer>,
    write: Arc<Writer<W>>,
}

impl<W> Releasing<W> {
    fn release(&self) {
        self.write.release();
    }
}

/// The handle the sender writes with, until the channel lets go of it.
struct Writer<W> {
    stream: Mutex<Option<W>>,
    released: AtomicBool,
}

impl<W> Writer<W> {
    fn new(stream: W) -> Self {
        Self {
            stream: Mutex::new(Some(stream)),
            released: AtomicBool::new(false),
        }
    }

    /// Let go of the handle now, or, while a send holds it, once that send
    /// returns. Closing never waits for a send: one blocked on a far end
    /// that stopped reading need not wake when the stream is shut down
    /// (Winsock), and whoever closes may be what ends it (a process closer
    /// ends the process next).
    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        let stream = match self.stream.try_lock() {
            Ok(mut stream) => stream.take(),
            Err(TryLockError::Poisoned(stream)) => stream.into_inner().take(),
            // The send checks `released` after it lets go of the lock.
            Err(TryLockError::WouldBlock) => None,
        };
        drop(stream);
    }

    /// After a send let go of the lock: a release that found it held left
    /// the handle to the send.
    fn released_meanwhile(&self) {
        if self.released.load(Ordering::SeqCst) {
            let stream = self
                .stream
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            drop(stream);
        }
    }
}

impl<W: Send> Closer for Releasing<W> {
    fn close(&self, reason: &str) {
        self.closer.close(reason);
        self.release();
    }

    fn replaced(&self) {
        self.closer.replaced();
        self.release();
    }

    fn rejected(&self, reason: &str) {
        self.closer.rejected(reason);
        self.release();
    }
}

/// Whether the framing refuses `message` itself, without it reaching the
/// stream: over [`MAX_MESSAGE`] (which also ends the channel), or holding a
/// raw newline.
pub(crate) fn refuses(message: &[u8]) -> bool {
    message.len() > MAX_MESSAGE || message.contains(&b'\n')
}

/// What both directions share: whether the channel ended, how to end it,
/// and the size limit.
#[derive(Clone)]
struct End {
    ended: Arc<AtomicBool>,
    closer: Arc<dyn Closer>,
    max_message: usize,
}

impl End {
    /// A message over the limit ends the channel either way, as a WebSocket
    /// closes with 1009.
    fn too_big(&self, reason: String) -> ChannelError {
        self.ended.store(true, Ordering::SeqCst);
        self.closer.close(&reason);
        closed(reason)
    }
}

/// The channel has no half-close: once its receiver saw the end, sending
/// fails too. A stream does not say so by itself everywhere: on macOS a
/// socket whose far end shut it down still takes writes, and drops them.
struct LineSender<W>(Arc<Writer<W>>, End);

impl<W: Write + Send> Sender for LineSender<W> {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError> {
        if self.1.ended.load(Ordering::SeqCst) {
            return Err(closed("the channel ended"));
        }
        if message.len() > self.1.max_message {
            return Err(self.1.too_big(format!(
                "message of {} bytes exceeds the limit of {}",
                message.len(),
                self.1.max_message
            )));
        }
        if message.contains(&b'\n') {
            return Err(closed("message contains a raw newline"));
        }
        // Taken only now: refusing a message over the limit closes the
        // channel, which takes the stream.
        let sent = write_line(&self.0, message);
        self.0.released_meanwhile();
        sent
    }
}

/// `message` and its newline, on the writer's handle while it has one.
fn write_line<W: Write>(writer: &Writer<W>, message: &[u8]) -> Result<(), ChannelError> {
    let mut write = writer.stream.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(stream) = write
        .as_mut()
        .filter(|_| !writer.released.load(Ordering::SeqCst))
    else {
        return Err(closed("the channel ended"));
    };
    // The message and its newline in one write where the stream takes
    // both, without copying the message.
    let mut parts = [IoSlice::new(message), IoSlice::new(b"\n")];
    let mut parts = &mut parts[..];
    while !parts.is_empty() {
        match stream.write_vectored(parts) {
            Ok(0) => return Err(closed("stream closed while sending")),
            Ok(written) => IoSlice::advance_slices(&mut parts, written),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(closed(error)),
        }
    }
    stream.flush().map_err(closed)
}

/// The stream, until the receiver saw its end.
struct LineReceiver<R>(Option<BufReader<R>>, End);

impl<R: Read + Send> LineReceiver<R> {
    /// The next line, failing as soon as it outgrows the limit: never more
    /// than the limit and one buffer is read ahead of a newline.
    fn line(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
        let max = self.1.max_message;
        let Some(stream) = self.0.as_mut() else {
            return Ok(None);
        };
        let mut line = Vec::new();
        loop {
            let available = match stream.fill_buf() {
                Ok(available) => available,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(closed(error)),
            };
            if available.is_empty() {
                return match line.is_empty() {
                    true => Ok(None),
                    false => Err(closed("stream ended inside a message")),
                };
            }
            let newline = available.iter().position(|&byte| byte == b'\n');
            let part = &available[..newline.unwrap_or(available.len())];
            if line.len() + part.len() > max {
                return Err(self
                    .1
                    .too_big(format!("received a message over the limit of {max} bytes")));
            }
            line.extend_from_slice(part);
            let used = part.len() + usize::from(newline.is_some());
            stream.consume(used);
            if newline.is_some() {
                return Ok(Some(line));
            }
        }
    }
}

impl<R: Read + Send> Receiver for LineReceiver<R> {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
        let received = self.line();
        if !matches!(received, Ok(Some(_))) {
            self.1.ended.store(true, Ordering::SeqCst);
            // All the stream had left to say ([`Releasing`]).
            self.0 = None;
        }
        received
    }
}

fn closed(reason: impl std::fmt::Display) -> ChannelError {
    ChannelError::Closed {
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Nothing;
    impl Closer for Nothing {
        fn close(&self, _: &str) {}
    }

    fn receive(bytes: &[u8]) -> Channel {
        channel(
            Cursor::new(bytes.to_vec()),
            Vec::new(),
            Arc::new(Nothing),
            ChannelInfo::default(),
        )
    }

    #[test]
    fn strips_one_newline_per_message() {
        let mut channel = receive(b"{\"a\":1}\n\n{}\n");
        assert_eq!(channel.receiver.recv().unwrap().unwrap(), b"{\"a\":1}");
        assert_eq!(channel.receiver.recv().unwrap().unwrap(), b"");
        assert_eq!(channel.receiver.recv().unwrap().unwrap(), b"{}");
        assert_eq!(channel.receiver.recv().unwrap(), None);
    }

    #[test]
    fn a_truncated_message_is_an_error_not_an_end() {
        let mut channel = receive(b"{\"a\"");
        assert!(matches!(
            channel.receiver.recv(),
            Err(ChannelError::Closed { .. })
        ));
    }

    #[test]
    fn appends_the_newline_and_refuses_raw_ones() {
        let stream = Arc::new(Writer::new(Vec::new()));
        let mut sender = LineSender(
            stream.clone(),
            End {
                ended: Arc::default(),
                closer: Arc::new(Nothing),
                max_message: MAX_MESSAGE,
            },
        );
        let written = || stream.stream.lock().unwrap().clone().unwrap();
        sender.send(b"{}").unwrap();
        assert_eq!(written(), b"{}\n");
        assert!(sender.send(b"a\nb").is_err());
        assert_eq!(written(), b"{}\n");
    }

    /// Bytes from `prefix`, then `x` for ever, counting what was read.
    struct Endless {
        prefix: Vec<u8>,
        read: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Read for Endless {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let at = self.read.load(Ordering::SeqCst);
            for (i, byte) in buffer.iter_mut().enumerate() {
                *byte = self.prefix.get(at + i).copied().unwrap_or(b'x');
            }
            self.read.fetch_add(buffer.len(), Ordering::SeqCst);
            Ok(buffer.len())
        }
    }

    fn over_the_limit(result: Result<Option<Vec<u8>>, ChannelError>) -> bool {
        matches!(&result, Err(ChannelError::Closed { reason }) if reason.contains("limit"))
    }

    /// Risk P2 (Q5.3.4, Q6.3.2): a line over the limit closes the channel
    /// with a reason; the message is not delivered.
    #[test]
    fn a_line_over_the_limit_closes_the_channel() {
        let mut stream = vec![b'x'; MAX_MESSAGE + 1];
        stream.extend_from_slice(b"\n{}\n");
        let mut channel = receive(&stream);
        assert!(over_the_limit(channel.receiver.recv()));
        assert!(channel.sender.send(b"{}").is_err(), "the channel ended");
    }

    /// Risk P2: a far end that never sends a newline does not make the
    /// receiver read (and keep) without end.
    #[test]
    fn endless_bytes_without_a_newline_stop_at_the_limit() {
        let read = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let endless = Endless {
            prefix: Vec::new(),
            read: read.clone(),
        };
        let mut channel = channel(
            endless,
            Vec::new(),
            Arc::new(Nothing),
            ChannelInfo::default(),
        );
        assert!(over_the_limit(channel.receiver.recv()));
        // Read at most the limit and one buffer more.
        assert!(read.load(Ordering::SeqCst) <= MAX_MESSAGE + 64 * 1024);
    }

    /// Q6.3.2: the boundary values, exactly at the limit and one byte over,
    /// with the limit at its default and set low.
    #[test]
    fn a_line_exactly_at_the_limit_arrives_one_byte_more_does_not() {
        let mut stream = vec![b'a'; MAX_MESSAGE];
        stream.push(b'\n');
        let mut channel = receive(&stream);
        assert_eq!(channel.receiver.recv().unwrap().unwrap().len(), MAX_MESSAGE);
        assert_eq!(channel.receiver.recv().unwrap(), None);

        for (length, delivered) in [(0, true), (16, true), (17, false)] {
            let mut stream = vec![b'b'; length];
            stream.push(b'\n');
            let mut channel = limited(
                Cursor::new(stream),
                Vec::new(),
                Arc::new(Nothing),
                ChannelInfo::default(),
                16,
            );
            let received = channel.receiver.recv();
            if delivered {
                assert_eq!(received.unwrap().unwrap().len(), length);
            } else {
                assert!(over_the_limit(received), "{length} bytes");
            }
        }
    }

    /// The limit holds when a line arrives in reads of any size: the
    /// newline at the very end of a read, or in the next one.
    #[test]
    fn the_limit_counts_bytes_across_reads() {
        struct Trickle(std::collections::VecDeque<Vec<u8>>);
        impl Read for Trickle {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let Some(chunk) = self.0.pop_front() else {
                    return Ok(0);
                };
                buffer[..chunk.len()].copy_from_slice(&chunk);
                Ok(chunk.len())
            }
        }
        let chunks = |parts: &[&[u8]]| Trickle(parts.iter().map(|p| p.to_vec()).collect());
        let open = |reads| {
            limited(
                reads,
                Vec::new(),
                Arc::new(Nothing),
                ChannelInfo::default(),
                8,
            )
        };
        let mut at_limit = open(chunks(&[b"abcd", b"efgh", b"\n"]));
        assert_eq!(at_limit.receiver.recv().unwrap().unwrap(), b"abcdefgh");
        let mut over = open(chunks(&[b"abcd", b"efgh", b"i\n"]));
        assert!(over_the_limit(over.receiver.recv()));
    }

    /// Only `\n` separates messages: a `\r` before it is a byte of the
    /// message, and counts toward the limit (JSON takes it as trailing
    /// whitespace). The same in Node and Python.
    #[test]
    fn a_carriage_return_is_part_of_the_message() {
        let open = |bytes: &[u8]| {
            limited(
                Cursor::new(bytes.to_vec()),
                Vec::new(),
                Arc::new(Nothing),
                ChannelInfo::default(),
                4,
            )
        };
        assert_eq!(open(b"{}\r\n").receiver.recv().unwrap().unwrap(), b"{}\r");
        assert_eq!(open(b"abc\r\n").receiver.recv().unwrap().unwrap(), b"abc\r");
        assert!(over_the_limit(open(b"abcd\r\n").receiver.recv()));
    }

    /// Sending over the limit is refused, and ends the channel as the far
    /// end would on receiving it.
    #[test]
    fn sending_over_the_limit_is_refused_and_closes() {
        struct Count(Arc<std::sync::atomic::AtomicUsize>);
        impl Closer for Count {
            fn close(&self, _: &str) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let closed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut channel = limited(
            Cursor::new(Vec::new()),
            Vec::new(),
            Arc::new(Count(closed.clone())),
            ChannelInfo::default(),
            4,
        );
        channel.sender.send(b"1234").unwrap();
        let refused = channel.sender.send(b"12345");
        assert!(
            matches!(&refused, Err(ChannelError::Closed { reason }) if reason.contains("limit")),
            "{refused:?}"
        );
        assert_eq!(closed.load(Ordering::SeqCst), 1);
        assert!(channel.sender.send(b"1").is_err(), "the channel ended");
    }

    /// A stream that takes every write after its far end went (as a macOS
    /// socket does): the channel still refuses to send once it saw the end.
    #[test]
    fn sending_fails_once_the_end_was_received() {
        let mut channel = receive(b"{}\n");
        assert_eq!(channel.receiver.recv().unwrap().unwrap(), b"{}");
        channel.sender.send(b"{}").unwrap();
        assert_eq!(channel.receiver.recv().unwrap(), None);
        assert!(channel.sender.send(b"{}").is_err());
    }

    /// A handle of the stream, which says when it was let go of.
    struct Handle<T>(T, Arc<AtomicBool>);
    impl<T> Drop for Handle<T> {
        fn drop(&mut self) {
            self.1.store(true, Ordering::SeqCst);
        }
    }
    impl<T: Read> Read for Handle<T> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buffer)
        }
    }
    impl<T: Write> Write for Handle<T> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.write(bytes)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.flush()
        }
    }

    /// The far end of a stream hears that the channel ended when this side's
    /// last handle of it goes (a shutdown does not always tell it, on
    /// macOS): the channel lets go of its writing handle once closed, and of
    /// its reading one once the receiver saw the end.
    #[test]
    fn the_stream_is_let_go_of_once_the_channel_ended() {
        let (read, written) = (Arc::default(), Arc::default());
        let mut channel = channel(
            Handle(Cursor::new(b"{}\n".to_vec()), Arc::clone(&read)),
            Handle(Vec::new(), Arc::clone(&written)),
            Arc::new(Nothing),
            ChannelInfo::default(),
        );
        channel.sender.send(b"{}").unwrap();
        assert_eq!(channel.receiver.recv().unwrap().unwrap(), b"{}");
        assert!(!read.load(Ordering::SeqCst) && !written.load(Ordering::SeqCst));
        channel.closer.close("done");
        assert!(written.load(Ordering::SeqCst), "the writing handle went");
        assert!(channel.sender.send(b"{}").is_err());
        assert_eq!(channel.receiver.recv().unwrap(), None);
        assert!(read.load(Ordering::SeqCst), "the reading handle went");
        assert_eq!(channel.receiver.recv().unwrap(), None, "and it stays ended");
    }

    /// A write that blocks until `go` says so, as a send to a far end that
    /// stopped reading does where shutting the stream down does not wake it.
    struct Stuck(std::sync::mpsc::Receiver<()>, Arc<AtomicBool>);
    impl Write for Stuck {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            self.1.store(true, Ordering::SeqCst);
            let _ = self.0.recv();
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Closing does not wait for a blocked send: the handle goes once the
    /// send returns (what closes may be what ends it, as a process closer
    /// ending its process).
    #[test]
    fn closing_does_not_wait_for_a_blocked_send() {
        let (go, wait) = std::sync::mpsc::channel();
        let (blocked, written) = (Arc::new(AtomicBool::new(false)), Arc::default());
        let Channel {
            mut sender, closer, ..
        } = channel(
            Cursor::new(Vec::new()),
            Handle(Stuck(wait, blocked.clone()), Arc::clone(&written)),
            Arc::new(Nothing),
            ChannelInfo::default(),
        );
        let sending = std::thread::spawn(move || sender.send(b"{}"));
        while !blocked.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        closer.close("done");
        assert!(!written.load(Ordering::SeqCst), "the send still holds it");
        go.send(()).unwrap();
        assert!(sending.join().unwrap().is_err());
        assert!(written.load(Ordering::SeqCst), "the send let go of it");
    }

    /// A far end that stopped reading, on the loopback connection a spawned
    /// process gets on Windows: with the send blocked on a full buffer,
    /// closing returns at once.
    #[test]
    fn closing_returns_while_a_send_waits_on_a_far_end_that_does_not_read() {
        use std::net::{Shutdown, TcpListener, TcpStream};
        struct ShutTcp(TcpStream);
        impl Closer for ShutTcp {
            fn close(&self, _: &str) {
                let _ = self.0.shutdown(Shutdown::Both);
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (far_end, _) = listener.accept().unwrap();
        let Channel {
            mut sender, closer, ..
        } = channel(
            stream.try_clone().unwrap(),
            stream.try_clone().unwrap(),
            Arc::new(ShutTcp(stream)),
            ChannelInfo::default(),
        );
        let message = vec![b'x'; 1 << 20];
        let sending = std::thread::spawn(move || while sender.send(&message).is_ok() {});
        // Long enough for the buffers to fill and the send to block.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let (closed, closing) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            closer.close("done");
            let _ = closed.send(());
        });
        let returned = closing.recv_timeout(std::time::Duration::from_secs(5));
        // The far end going ends the send either way.
        drop(far_end);
        sending.join().unwrap();
        assert!(returned.is_ok(), "close waited for the blocked send");
    }
}

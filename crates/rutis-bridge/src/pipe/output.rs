//! A started process's output passed on a whole line at a time
//! ([`Stdio::Lines`](super::Stdio::Lines)), and this process's own lines
//! written the same way ([`write_line`]).
//!
//! A process that shares this process's output writes when it likes: a
//! Python `print` writes its text and its newline separately, and a line
//! this process writes in between lands inside the plugin's (#332). So the
//! process writes into a pipe instead, and a thread here reads it and
//! writes each complete line to this process's stream in one write, under
//! the locks every line written here takes. The thread reads all the time,
//! whatever the line: a process that writes a lot is never blocked by an
//! unread pipe, only by this process's own output when that is blocked.

use std::io::{BufRead, Read, Write};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// One of this process's output streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// A line longer than this, without its newline yet, is passed on in pieces
/// of this size: the reader does not hold more than this of one line.
pub const MAX_LINE: usize = 64 * 1024;

/// Write `line` and a newline to this process's `stream` in one write,
/// holding the locks of both stdout and stderr (in that order), as every
/// line passed on from a process started with
/// [`Stdio::Lines`](super::Stdio::Lines) is: no such line, nor another line
/// written this way, lands inside it, also when stdout and stderr are the
/// same file (`2>&1`). An error (a closed stream) is ignored, as there is
/// nowhere left to say it.
pub fn write_line(stream: OutputStream, line: &str) {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    emit(stream, &bytes);
}

/// Write `bytes` to `stream` as one write, under both streams' locks.
fn emit(stream: OutputStream, bytes: &[u8]) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let stderr = std::io::stderr();
    let mut err = stderr.lock();
    let written = match stream {
        OutputStream::Stdout => out.write_all(bytes).and_then(|()| out.flush()),
        OutputStream::Stderr => err.write_all(bytes).and_then(|()| err.flush()),
    };
    // A Windows console takes only UTF-8: what a process wrote in another
    // encoding is passed on with its invalid bytes replaced.
    if let Err(error) = written {
        if error.kind() == std::io::ErrorKind::InvalidData {
            let text = String::from_utf8_lossy(bytes);
            let _ = match stream {
                OutputStream::Stdout => out.write_all(text.as_bytes()).and_then(|()| out.flush()),
                OutputStream::Stderr => err.write_all(text.as_bytes()),
            };
        }
    }
}

/// The pipes being passed on, until each has ended and its last line is
/// written.
static OPEN: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// A pipe for a process's `stream`: the end to give the process. A thread
/// passes on what it writes, a line at a time, each after `prefix` if any,
/// until every process holding the pipe has closed it (the process, and
/// any process it started that kept the stream).
pub(crate) fn pipe(
    stream: OutputStream,
    prefix: Option<&str>,
) -> std::io::Result<std::process::Stdio> {
    let (reader, writer) = std::io::pipe()?;
    let prefix = prefix.map(|prefix| prefix.as_bytes().to_vec());
    *OPEN.0.lock().unwrap() += 1;
    let started = std::thread::Builder::new()
        .name("rutis-output".into())
        .spawn(move || {
            forward(reader, prefixed(prefix, |bytes| emit(stream, bytes)));
            let mut open = OPEN.0.lock().unwrap();
            *open -= 1;
            OPEN.1.notify_all();
        });
    if let Err(error) = started {
        *OPEN.0.lock().unwrap() -= 1;
        return Err(error);
    }
    Ok(writer.into())
}

/// Wait up to `guard` until the output of every process started with
/// [`Stdio::Lines`](super::Stdio::Lines) has ended and all of it has been
/// written: what a host that is ending does once its processes have ended
/// ([`processes_ended`](super::processes_ended)), so that their last lines
/// are not lost when it exits. False when some is still open after `guard`:
/// a process they started that is still running holds it.
pub fn output_ended(guard: Duration) -> bool {
    let (open, ended) = &OPEN;
    let (open, _) = ended
        .wait_timeout_while(open.lock().unwrap(), guard, |open| *open > 0)
        .unwrap();
    *open == 0
}

/// `emit`, given each line (or piece of a long line) after `prefix`: a
/// line's first piece starts with it, in the same write.
pub(crate) fn prefixed(prefix: Option<Vec<u8>>, mut emit: impl FnMut(&[u8])) -> impl FnMut(&[u8]) {
    let mut at_start = true;
    move |bytes: &[u8]| {
        match (&prefix, at_start) {
            (Some(prefix), true) => {
                let mut line = Vec::with_capacity(prefix.len() + bytes.len());
                line.extend_from_slice(prefix);
                line.extend_from_slice(bytes);
                emit(&line);
            }
            _ => emit(bytes),
        }
        at_start = bytes.ends_with(b"\n");
    }
}

/// Read `reader` to its end, giving `emit` each line with its newline; a
/// last line without one gets one, and a line longer than [`MAX_LINE`] is
/// given in pieces of that size (without a newline but the last).
pub(crate) fn forward(reader: impl Read, mut emit: impl FnMut(&[u8])) {
    let mut reader = std::io::BufReader::with_capacity(MAX_LINE, reader);
    let mut line = Vec::new();
    loop {
        let (complete, used) = match reader.fill_buf() {
            Ok([]) => break,
            Ok(bytes) => {
                let room = MAX_LINE - line.len();
                let bytes = &bytes[..bytes.len().min(room)];
                match bytes.iter().position(|byte| *byte == b'\n') {
                    Some(end) => {
                        line.extend_from_slice(&bytes[..=end]);
                        (true, end + 1)
                    }
                    None => {
                        line.extend_from_slice(bytes);
                        (line.len() == MAX_LINE, bytes.len())
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        reader.consume(used);
        if complete {
            emit(&line);
            line.clear();
        }
    }
    if !line.is_empty() {
        line.push(b'\n');
        emit(&line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader giving its chunks one read at a time, as a pipe gives
    /// separate writes.
    struct Chunks(std::collections::VecDeque<Vec<u8>>);

    impl Read for Chunks {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let Some(mut chunk) = self.0.pop_front() else {
                return Ok(0);
            };
            let n = chunk.len().min(buf.len());
            buf[..n].copy_from_slice(&chunk[..n]);
            if n < chunk.len() {
                self.0.push_front(chunk.split_off(n));
            }
            Ok(n)
        }
    }

    fn lines(chunks: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let chunks = Chunks(chunks.iter().map(|chunk| chunk.to_vec()).collect());
        forward(chunks, |line| out.push(line.to_vec()));
        out
    }

    /// guarantee: host/output-lines-whole (#332): a line written in pieces
    /// is passed on whole, once its newline comes.
    #[test]
    fn a_line_written_in_pieces_is_passed_on_whole() {
        assert_eq!(
            lines(&[b"{\"probe\": 1}", b"\n", b"a\nb", b"c\n"]),
            [&b"{\"probe\": 1}\n"[..], b"a\n", b"bc\n"]
        );
    }

    /// guarantee: host/output-prefix (#678): each line starts with the
    /// prefix, written with it in one piece; a long line's pieces after
    /// the first do not.
    #[test]
    fn each_line_is_passed_on_after_the_prefix() {
        let mut out = Vec::new();
        let long = vec![b'x'; MAX_LINE + 2];
        let mut written = long.clone();
        written.push(b'\n');
        let chunks = Chunks(
            [&b"a\nb"[..], b"c\n", &written]
                .iter()
                .map(|chunk| chunk.to_vec())
                .collect(),
        );
        forward(
            chunks,
            prefixed(Some(b"[go-w] ".to_vec()), |line| out.push(line.to_vec())),
        );
        let mut first = b"[go-w] ".to_vec();
        first.extend(vec![b'x'; MAX_LINE]);
        assert_eq!(
            out,
            [
                b"[go-w] a\n".to_vec(),
                b"[go-w] bc\n".to_vec(),
                first,
                b"xx\n".to_vec()
            ]
        );
    }

    #[test]
    fn a_last_line_without_a_newline_gets_one_at_the_end() {
        assert_eq!(lines(&[b"a\n", b"partial"]), [&b"a\n"[..], b"partial\n"]);
        assert!(lines(&[]).is_empty());
    }

    #[test]
    fn line_ends_are_kept_as_written() {
        assert_eq!(lines(&[b"a\r", b"\nb\n"]), [&b"a\r\n"[..], b"b\n"]);
    }

    #[test]
    fn a_line_longer_than_the_limit_is_passed_on_in_pieces() {
        let long = vec![b'x'; MAX_LINE + 10];
        let mut written = long.clone();
        written.push(b'\n');
        let out = lines(&[&written]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![b'x'; MAX_LINE]);
        assert_eq!(out[1], b"xxxxxxxxxx\n");
    }
}

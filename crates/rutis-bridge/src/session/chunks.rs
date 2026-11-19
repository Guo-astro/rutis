//! A frame over the transport's per-message limit, carried in `chunk` frames
//! and reassembled by the far end (#1361; session protocol §2.5).
//!
//! A `chunk` frame is not a [`Frame`](crate::session::protocol::Frame)
//! variant: the session's read layer takes it before `Frame` decoding, so the
//! frame vocabulary and every rule about call ids, references and unknown
//! fields stay as they are. The sending side cuts an encoded frame's bytes,
//! base64s each slice, and queues the slices together; the receiving side
//! joins them and hands the bytes back to the session as one message.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::session::values::{base64_decode, base64_encode};
use crate::session::Error;

/// The capability by which a side says it reassembles `chunk` frames.
pub(crate) const CHUNKED: &str = "chunked";

/// The most bytes one `chunk` frame carries before base64: 8 MiB, about
/// 10.67 MiB on the wire, inside the 16 MiB a process pipe or a WebSocket
/// takes by default.
pub const CHUNK_SIZE: usize = 8 << 20;

/// The default most a chunked transfer carries once reassembled, and the
/// default most one frame may be on a session: 256 MiB. A host raises or
/// lowers it per session with `SessionOptions::max_transfer` (#1361).
pub const DEFAULT_MAX_TRANSFER: usize = 256 << 20;

/// The room a `chunk` frame needs beyond its base64 `data`: the tag, the
/// transfer id, the sequence number, the flag and the JSON syntax.
const FRAME_ROOM: usize = 256;

/// The largest slice whose `chunk` frame, base64 included, fits `limit`
/// bytes; 0 when not even one byte does. A smaller transport limit shrinks
/// the slice, so a chunk frame never exceeds the transport itself.
pub(crate) fn slice_size(limit: usize) -> usize {
    (limit.saturating_sub(FRAME_ROOM) / 4 * 3).min(CHUNK_SIZE)
}

/// One `chunk` frame: a slice of a frame's encoded bytes.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Chunk {
    /// Always `"chunk"`; the reader checks the `op` before decoding.
    pub op: String,
    /// `<sending endpoint>:<n>`, unique among this sender's transfers.
    pub transfer: String,
    /// From 0, one more for each slice.
    pub seq: u64,
    /// The last slice of the transfer.
    pub last: bool,
    /// The base64 of one slice of the frame's UTF-8 JSON bytes.
    pub data: String,
}

impl Chunk {
    pub(crate) fn new(
        transfer: impl Into<String>,
        seq: u64,
        last: bool,
        data: impl AsRef<[u8]>,
    ) -> Self {
        Self {
            op: "chunk".into(),
            transfer: transfer.into(),
            seq,
            last,
            data: base64_encode(data.as_ref()),
        }
    }

    /// `bytes` cut into `chunk` frames that each fit `limit`; `last` is on
    /// the final one.
    pub(crate) fn frames(bytes: &[u8], limit: usize, transfer: &str) -> Vec<Self> {
        let size = slice_size(limit).max(1);
        let slices: Vec<&[u8]> = bytes.chunks(size).collect();
        let last = slices.len().saturating_sub(1);
        if slices.is_empty() {
            return vec![Self::new(transfer, 0, true, [])];
        }
        slices
            .into_iter()
            .enumerate()
            .map(|(seq, slice)| Self::new(transfer, seq as u64, seq == last, slice))
            .collect()
    }

    /// The `chunk` frame `value` is, or `None` when it is another frame.
    /// A `chunk` with fields beyond its own is a malformed frame, which the
    /// reader treats as a transport error.
    pub(crate) fn of(value: &Json) -> Result<Option<Self>, Error> {
        if value.get("op").and_then(Json::as_str) != Some("chunk") {
            return Ok(None);
        }
        let chunk =
            Self::deserialize(value).map_err(|error| Error::Transport(error.to_string()))?;
        Ok(Some(chunk))
    }
}

/// The transfer a receiver is in the middle of, if any. One at a time: the
/// sender queues a frame's chunks together, so another transfer's chunk can
/// only mean the far end broke the rule.
pub(crate) struct Reassembly {
    limit: usize,
    transfer: Option<Transfer>,
}

struct Transfer {
    id: String,
    next: u64,
    bytes: Vec<u8>,
}

/// `over the limit of <n> bytes (<n> MiB)` for the reassembly cap.
fn over_limit(limit: usize) -> String {
    const MIB: usize = 1 << 20;
    let mib = match limit % MIB {
        0 => format!(" ({} MiB)", limit / MIB),
        _ => String::new(),
    };
    format!("over the limit of {limit} bytes{mib}")
}

impl Reassembly {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            transfer: None,
        }
    }

    /// Buffer `chunk`; `Ok(Some(bytes))` once its transfer is complete.
    /// Every violation is a protocol error: there is no call to fail.
    pub(crate) fn accept(&mut self, chunk: Chunk) -> Result<Option<Vec<u8>>, Error> {
        if chunk.transfer.is_empty() {
            return Err(Error::Transport("chunk frame without a transfer id".into()));
        }
        let limit = self.limit;
        let id = chunk.transfer;
        match &mut self.transfer {
            Some(current) if current.id != id => Err(Error::Transport(format!(
                "chunked transfer {}: transfer {id} started before it ended",
                current.id
            ))),
            Some(current) => {
                if chunk.seq != current.next {
                    return Err(Error::Transport(format!(
                        "chunked transfer {}: expected chunk {}, got {}",
                        current.id, current.next, chunk.seq
                    )));
                }
                let part = decode(&id, &chunk.data)?;
                current.bytes.extend_from_slice(&part);
                if current.bytes.len() > limit {
                    return Err(Error::Transport(format!(
                        "chunked transfer {}: {}",
                        current.id,
                        over_limit(limit)
                    )));
                }
                current.next += 1;
                if !chunk.last {
                    return Ok(None);
                }
                let done = self.transfer.take().expect("a transfer in progress").bytes;
                Ok(Some(done))
            }
            None => {
                if chunk.seq != 0 {
                    return Err(Error::Transport(format!(
                        "chunked transfer {id}: expected chunk 0, got {}",
                        chunk.seq
                    )));
                }
                let part = decode(&id, &chunk.data)?;
                if part.len() > limit {
                    return Err(Error::Transport(format!(
                        "chunked transfer {id}: {}",
                        over_limit(limit)
                    )));
                }
                self.transfer = Some(Transfer {
                    id,
                    next: 1,
                    bytes: part,
                });
                if !chunk.last {
                    return Ok(None);
                }
                let done = self.transfer.take().expect("a transfer in progress").bytes;
                Ok(Some(done))
            }
        }
    }
}

/// The bytes of a chunk's `data`, saying which transfer it broke.
fn decode(transfer: &str, data: &str) -> Result<Vec<u8>, Error> {
    base64_decode(data)
        .map_err(|error| Error::Transport(format!("chunked transfer {transfer}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame's slices fit the transport limit and join back to the frame.
    #[test]
    fn slices_fit_the_limit_and_join_back() {
        let bytes: Vec<u8> = (0..4096u32).map(|n| n as u8).collect();
        for limit in [1024, 4096] {
            let chunks = Chunk::frames(&bytes, limit, "rust:1");
            assert!(chunks.len() > 1, "one chunk for {} bytes", bytes.len());
            for chunk in &chunks {
                let encoded = serde_json::to_vec(chunk).unwrap();
                assert!(
                    encoded.len() <= limit,
                    "a chunk frame of {} bytes for a limit of {limit}",
                    encoded.len()
                );
            }
            let mut joined = Vec::new();
            for (seq, chunk) in chunks.iter().enumerate() {
                assert_eq!(chunk.seq, seq as u64);
                assert_eq!(chunk.last, seq == chunks.len() - 1);
                joined.extend(decode(&chunk.transfer, &chunk.data).unwrap());
            }
            assert_eq!(joined, bytes);
        }
    }

    /// The receiver takes one transfer, in order, up to its limit.
    #[test]
    fn a_transfer_is_taken_in_order_and_up_to_its_limit() {
        let mut assembly = Reassembly::new(8);
        let first = Chunk::new("rust:1", 0, false, b"abcd");
        assert!(assembly.accept(first).unwrap().is_none());
        let mut second = Chunk::new("rust:1", 1, true, b"efgh");
        assert_eq!(
            assembly.accept(second.clone()).unwrap().unwrap(),
            b"abcdefgh"
        );
        // Complete: the next transfer starts at 0.
        second.seq = 0;
        assert_eq!(assembly.accept(second).unwrap().unwrap(), b"efgh");

        let mut assembly = Reassembly::new(8);
        assembly
            .accept(Chunk::new("rust:1", 0, false, b"abcd"))
            .unwrap();
        let error = assembly
            .accept(Chunk::new("rust:1", 1, true, b"efghi"))
            .expect_err("a transfer over the limit");
        assert_eq!(
            error.to_string(),
            "chunked transfer rust:1: over the limit of 8 bytes"
        );
    }

    /// A chunk out of order, of another transfer, or not base64 is refused.
    #[test]
    fn a_broken_transfer_is_refused() {
        let mut assembly = Reassembly::new(1024);
        let error = assembly
            .accept(Chunk::new("rust:1", 1, false, b"x"))
            .expect_err("a chunk out of order");
        assert_eq!(
            error.to_string(),
            "chunked transfer rust:1: expected chunk 0, got 1"
        );

        assembly
            .accept(Chunk::new("rust:1", 0, false, b"x"))
            .unwrap();
        let error = assembly
            .accept(Chunk::new("rust:2", 0, false, b"y"))
            .expect_err("two transfers");
        assert_eq!(
            error.to_string(),
            "chunked transfer rust:1: transfer rust:2 started before it ended"
        );

        let mut broken = Chunk::new("rust:1", 0, true, b"{}");
        broken.data = "not base64!".into();
        let mut assembly = Reassembly::new(1024);
        let error = assembly.accept(broken).expect_err("bad base64");
        assert!(
            error
                .to_string()
                .starts_with("chunked transfer rust:1: invalid bytes (11 characters)"),
            "{error}"
        );
    }
}

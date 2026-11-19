"""A frame over the channel's per-message limit, carried by `chunk` frames
and reassembled by the far end (#1363; session protocol §2.5).

A `chunk` frame is not a session frame: the reader takes it before the frame
is decoded, so the frame vocabulary and every rule about call ids, references
and unknown fields stay as they are. The sending side cuts an encoded frame's
bytes, base64s each slice and writes the slices together; the receiving side
joins them and hands the bytes to the session as one message.
"""

from __future__ import annotations

import base64
import binascii
import json
import os
import re

# The most bytes one `chunk` frame carries before base64: 8 MiB, about
# 10.67 MiB on the wire, inside the 16 MiB a process pipe or WebSocket takes.
CHUNK_SIZE = 8 * 1024 * 1024

# The default most a chunked transfer carries once reassembled, and the
# default most one frame may be on a session: 256 MiB. A deployment raises or
# lowers it with `RUTIS_MAX_TRANSFER`; never unlimited.
DEFAULT_MAX_TRANSFER = 256 * 1024 * 1024
MAX_TRANSFER_ENV = "RUTIS_MAX_TRANSFER"

# The room a `chunk` frame needs beyond its base64 `data`: the tag, the
# transfer id, the sequence number, the flag and the JSON syntax.
FRAME_ROOM = 256

_POSITIVE = re.compile(r"^[0-9]+$")
_MIB = 1024 * 1024


def max_transfer_from_environment() -> int:
    """The session's reassembly cap: `RUTIS_MAX_TRANSFER`, or the default.
    A value that is not a positive number of bytes refuses the session,
    naming the variable and what it expected."""
    value = os.environ.get(MAX_TRANSFER_ENV)
    if value is None:
        return DEFAULT_MAX_TRANSFER
    if not _POSITIVE.match(value) or int(value) <= 0:
        raise ValueError(f"{MAX_TRANSFER_ENV} {json.dumps(value)}: expected a positive number of bytes")
    return int(value)


def chunk_size(limit: int) -> int:
    """The largest slice whose `chunk` frame, base64 included, fits `limit`
    bytes; 0 when not even one byte does. A smaller channel limit shrinks
    the slice, so a `chunk` frame never exceeds the channel itself."""
    return min(max((limit - FRAME_ROOM) // 4 * 3, 0), CHUNK_SIZE)


def over_limit(limit: int) -> str:
    """`over the limit of <n> bytes (<n> MiB)` for a transfer cap."""
    mib = f" ({limit // _MIB} MiB)" if limit % _MIB == 0 else ""
    return f"over the limit of {limit} bytes{mib}"


def encode_data(part: bytes) -> str:
    """One slice of a frame's bytes, as a `chunk` frame's `data`."""
    return base64.b64encode(part).decode("ascii")


def decode_data(text: str) -> bytes:
    """The bytes of a `chunk` frame's `data`: canonical base64 only (RFC
    4648 §4, padded, no whitespace), as rutis-bridge takes it."""
    try:
        data = base64.b64decode(text, validate=True)
    except (binascii.Error, ValueError):
        data = None
    if data is None or base64.b64encode(data).decode("ascii") != text:
        raise ValueError(f"invalid bytes ({len(text)} characters): expected base64 (RFC 4648 §4) with padding")
    return data

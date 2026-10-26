"""Channels a session runs on: ordered, reliable messages with their
boundaries kept, independent of the encoding of what they carry.

A channel has `send(message: bytes)`, `recv() -> bytes | None` (blocking;
`None` once the far end finished) and `close(reason)`. `recv` raises
`ConnectionError` when the channel failed.
"""

from __future__ import annotations

import socket
import threading

# The largest message either way, without its newline: the WebSocket
# binding's 16 MiB. Over it, the channel closes; a far end that never sends a
# newline costs at most this much memory.
MAX_MESSAGE = 16 * 1024 * 1024


class SocketChannel:
    """A Unix socket (dialed, or inherited as fd 3), one message per line of
    at most `max_message` bytes."""

    def __init__(self, sock: socket.socket, max_message: int = MAX_MESSAGE):
        self._sock = sock
        self._reader = sock.makefile("rb")
        self._lock = threading.Lock()
        self._max = max_message
        # Why the channel was closed, once it was: a send after that fails
        # with it, never on the closed descriptor (`Bad file descriptor`).
        self._closed: str | None = None

    @property
    def max_message(self) -> int:
        """The largest message either way: a session checks what it would
        send against it first, so one over it fails only its own call."""
        return self._max

    def send(self, message: bytes) -> None:
        if self._closed is not None:
            raise ConnectionError(f"the channel is closed{': ' + self._closed if self._closed else ''}")
        if len(message) > self._max:
            reason = f"message of {len(message)} bytes exceeds the limit of {self._max}"
            self.close(reason)
            raise ConnectionError(reason)
        if b"\n" in message:
            raise ValueError("a message contains a raw newline")
        with self._lock:
            self._sock.sendall(message + b"\n")

    def recv(self) -> bytes | None:
        # At most the limit and the newline: a longer line stops here.
        line = self._reader.readline(self._max + 1)
        if not line:
            return None
        if line.endswith(b"\n"):
            return line[:-1]
        if len(line) > self._max:
            reason = f"received a message over the limit of {self._max} bytes"
            self.close(reason)
            raise ConnectionError(reason)
        raise ConnectionError("stream ended inside a message")

    def close(self, reason: str = "") -> None:
        if self._closed is None:
            self._closed = reason
        try:
            self._sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        # The reader holds the descriptor open after the socket closes; a
        # read blocked on it returned once the socket was shut down.
        for closing in (self._reader, self._sock):
            try:
                closing.close()
            except OSError:
                pass

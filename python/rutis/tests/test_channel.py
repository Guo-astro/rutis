"""The newline framing of local channels: its size limit (risk P2; Q5.3.4,
Q6.3.2) and its boundaries."""

import errno
import importlib.util
import socket
import threading
import unittest

from rutis.channel import MAX_MESSAGE, SocketChannel
from support import HANG_GUARD as PATIENCE


class Writer(threading.Thread):
    """Writes `chunks` to `sock` on its own thread, then shuts its side down.
    With `cut`, the reader is expected to close the channel part way: the
    far end going away (on a send or on the shutdown) ends the writer. Any
    other failure, or that one without `cut`, fails the test that joins it."""

    GONE = {errno.EPIPE, errno.ECONNRESET, errno.ENOTCONN, errno.ESHUTDOWN}

    def __init__(self, sock: socket.socket, chunks, cut: bool = False):
        super().__init__(daemon=True)
        self.sock, self.chunks, self.cut, self.error = sock, chunks, cut, None

    def run(self):
        try:
            for chunk in self.chunks:
                self.sock.sendall(chunk)
            self.sock.shutdown(socket.SHUT_WR)
        except OSError as error:
            if not (self.cut and error.errno in self.GONE):
                self.error = error
        except BaseException as error:  # noqa: BLE001 - reported by finish()
            self.error = error

    def finish(self, case: unittest.TestCase):
        self.join(PATIENCE)
        case.assertFalse(self.is_alive(), "the writer is stuck")
        if self.error is not None:
            raise self.error


class LineLimitTests(unittest.TestCase):
    def pair(self, max_message=MAX_MESSAGE):
        ours, theirs = socket.socketpair()
        ours.settimeout(PATIENCE)
        theirs.settimeout(PATIENCE)
        self.addCleanup(ours.close)
        self.addCleanup(theirs.close)
        return SocketChannel(ours, max_message), theirs

    def test_the_default_limit_is_the_websocket_one(self):
        if importlib.util.find_spec("websockets") is None:
            self.skipTest("the websockets package (rutis[network]) is not installed")
        from rutis import websocket

        self.assertEqual(MAX_MESSAGE, websocket.MAX_MESSAGE)

    def test_a_line_at_the_limit_arrives_one_byte_more_closes(self):
        channel, theirs = self.pair()
        writer = Writer(theirs, [b"a" * MAX_MESSAGE + b"\n"])
        writer.start()
        self.assertEqual(len(channel.recv()), MAX_MESSAGE)
        self.assertIsNone(channel.recv())
        writer.finish(self)

        channel, theirs = self.pair()
        writer = Writer(theirs, [b"a" * (MAX_MESSAGE + 1) + b"\n{}\n"], cut=True)
        writer.start()
        with self.assertRaisesRegex(ConnectionError, "over the limit"):
            channel.recv()
        writer.finish(self)

    def test_a_line_at_the_limit_whose_newline_comes_in_a_later_read(self):
        channel, theirs = self.pair(max_message=8)
        theirs.sendall(b"1234")
        # Sent apart, received apart: the reader sees the line in pieces.
        writer = Writer(theirs, [b"5678", b"\n", b"123456789\n"], cut=True)
        writer.start()
        self.assertEqual(channel.recv(), b"12345678")
        with self.assertRaisesRegex(ConnectionError, "over the limit"):
            channel.recv()
        writer.finish(self)

    def test_a_carriage_return_is_part_of_the_message(self):
        # Only \n separates messages: a \r before it is a byte of the
        # message and counts toward the limit, as in Rust and Node.
        channel, theirs = self.pair(max_message=4)
        theirs.sendall(b"{}\r\nabc\r\nabcd\r\n")
        self.assertEqual(channel.recv(), b"{}\r")
        self.assertEqual(channel.recv(), b"abc\r")
        with self.assertRaisesRegex(ConnectionError, "over the limit"):
            channel.recv()

    def test_bytes_without_a_newline_stop_at_the_limit(self):
        channel, theirs = self.pair(max_message=1024)
        # Endless, unless the channel gives up.
        writer = Writer(theirs, iter(lambda: b"x" * 256, None), cut=True)
        writer.start()
        with self.assertRaisesRegex(ConnectionError, "over the limit of 1024 bytes"):
            channel.recv()
        writer.finish(self)

    def test_sending_over_the_limit_is_refused_and_closes(self):
        channel, theirs = self.pair(max_message=4)
        channel.send(b"1234")
        with self.assertRaisesRegex(ConnectionError, "exceeds the limit of 4"):
            channel.send(b"12345")
        lines = theirs.makefile("rb")
        self.assertEqual(lines.readline(), b"1234\n")
        self.assertEqual(lines.readline(), b"")

    def test_a_truncated_last_line_is_an_error_not_an_end(self):
        channel, theirs = self.pair(max_message=8)
        theirs.sendall(b"12345678\n{}")
        theirs.shutdown(socket.SHUT_WR)
        self.assertEqual(channel.recv(), b"12345678")
        with self.assertRaisesRegex(ConnectionError, "inside a message"):
            channel.recv()


if __name__ == "__main__":
    unittest.main()

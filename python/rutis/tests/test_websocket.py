"""The WebSocket binding of the Python runtime: authentication, the
subprotocol, close codes, the size limit and heartbeats."""

import functools
import inspect
import os
import queue
import socket
import ssl
import threading
import unittest
import unittest.mock
from urllib.parse import urlsplit

import background
from support import HANG_GUARD, blocking_within, eventually_blocking

try:
    from websockets.exceptions import ConnectionClosed, InvalidStatus
    from websockets.sync.client import connect

    from rutis import websocket
except ImportError:  # the optional `network` dependency is missing
    websocket = None

PROTOCOL = "rutis.2"


def listen(**options):
    """Start listen_once in a thread; return (address, channel queue)."""
    addresses: queue.Queue = queue.Queue()
    channels: queue.Queue = queue.Queue()

    def run():
        channels.put(
            websocket.listen_once(
                "ws://127.0.0.1:0/rutis",
                PROTOCOL,
                options.get("token", "secret"),
                announce=addresses.put,
            )
        )

    threading.Thread(target=run, daemon=True).start()
    return addresses.get(timeout=HANG_GUARD), channels


def dial(address, token="secret", protocol=PROTOCOL, **options):
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    return connect(address, additional_headers=headers, subprotocols=[protocol], **options)


@unittest.skipIf(websocket is None, "needs the websockets package (rutis[network])")
class WebSocketTest(background.TestCase):
    def test_messages_cross_and_an_orderly_close_is_a_normal_end(self):
        address, channels = listen()
        client = dial(address)
        channel = channels.get(timeout=HANG_GUARD)
        client.send('{"op":"hello"}')
        self.assertEqual(blocking_within("the message", channel.recv), b'{"op":"hello"}')
        channel.send(b'{"a":"x\\ny"}')
        self.assertEqual(client.recv(timeout=HANG_GUARD), '{"a":"x\\ny"}')
        client.close(1001)
        self.assertIsNone(blocking_within("the end", channel.recv))

    def test_a_closed_listener_refuses_new_connections_and_keeps_established_ones(self):
        # #197: before websockets 17, close() raised in a background thread
        # and the listener went on accepting.
        listener = websocket.listen("ws://127.0.0.1:0/rutis", PROTOCOL, "secret", announce=lambda _: None)
        first, second = dial(listener.address), dial(listener.address)
        channels = [listener.accept(), listener.accept()]
        listener.close()
        with self.assertRaises(ConnectionRefusedError):
            dial(listener.address)
        for client, channel in zip((first, second), channels):
            client.send("{}")
            self.assertEqual(blocking_within("the message", channel.recv), b"{}")
            channel.send(b"[]")
            self.assertEqual(client.recv(timeout=HANG_GUARD), "[]")
            client.close()
            self.assertIsNone(blocking_within("the end", channel.recv))

    def test_closing_twice_stops_once(self):
        listener = websocket.listen("ws://127.0.0.1:0/rutis", PROTOCOL, "secret", announce=lambda _: None)
        stops = []
        shutdown = listener._server.shutdown

        # functools.wraps keeps the signature close() probes, so each
        # websockets version takes its own branch.
        @functools.wraps(shutdown)
        def counted(*args, **kwargs):
            stops.append(kwargs)
            return shutdown(*args, **kwargs)

        listener._server.shutdown = counted
        listener.close()
        listener.close()
        takes_argument = "close_connections" in inspect.signature(shutdown).parameters
        self.assertEqual(stops, [{"close_connections": False} if takes_argument else {}])
        with self.assertRaises(ConnectionRefusedError):
            dial(listener.address)

    def test_a_listener_whose_serving_failed_refuses_and_close_raises(self):
        # Review of #197: an exception out of serve_forever() (its selector
        # failing, say; a failed accept() only makes it return) must not
        # leave a socket that takes connections nobody serves, nor a
        # close() that reports success. The mock raises from serve_forever.
        real_serve = websocket.serve

        def failing_serve(*args, **kwargs):
            server = real_serve(*args, **kwargs)

            def serve_forever():
                try:
                    raise RuntimeError("the selector failed")
                finally:
                    # What the real serve_forever() does on its way out;
                    # websockets 17's shutdown() waits for it.
                    if hasattr(server, "socket_closed"):
                        server.socket_closed.set()

            server.serve_forever = serve_forever
            return server

        websocket.serve = failing_serve
        try:
            listener = websocket.listen("ws://127.0.0.1:0/rutis", PROTOCOL, "secret", announce=lambda _: None)
        finally:
            websocket.serve = real_serve
        with self.assertRaisesRegex(RuntimeError, "the selector failed"):
            listener.close()
        with self.assertRaises(ConnectionRefusedError):
            dial(listener.address)
        # No thread is left behind; the serving one still reports the
        # exception as uncaught, by design.
        for thread in (listener._serving, listener._stopping):
            if thread is not None:
                thread.join(timeout=10)
                self.assertFalse(thread.is_alive(), thread.name)
        self.assertEqual(len(background.expected()), 1)

    def test_accept_raises_once_a_failed_accept_stops_serving(self):
        # #196 (left from #218): serve_forever() returns when accept() fails
        # (EMFILE, say) and websockets drops the error; accept() then
        # blocked for ever. Here serve_forever returns at once, as it does.
        real_serve = websocket.serve

        def returning_serve(*args, **kwargs):
            server = real_serve(*args, **kwargs)

            def serve_forever():
                if hasattr(server, "socket_closed"):
                    server.socket_closed.set()

            server.serve_forever = serve_forever
            return server

        websocket.serve = returning_serve
        try:
            listener = websocket.listen("ws://127.0.0.1:0/rutis", PROTOCOL, "secret", announce=lambda _: None)
        finally:
            websocket.serve = real_serve
        raised: queue.Queue = queue.Queue()

        def accept():
            try:
                listener.accept()
            except BaseException as error:  # noqa: BLE001 - what the test checks
                raised.put(error)

        threading.Thread(target=accept, daemon=True).start()
        error = raised.get(timeout=10)
        self.assertIsInstance(error, OSError)
        self.assertIn("stopped", str(error))
        with self.assertRaisesRegex(OSError, "stopped"):
            listener.accept()
        with self.assertRaisesRegex(OSError, "stopped"):
            listener.close()
        with self.assertRaises(ConnectionRefusedError):
            dial(listener.address)

    def test_accept_after_close_raises(self):
        listener = websocket.listen("ws://127.0.0.1:0/rutis", PROTOCOL, "secret", announce=lambda _: None)
        listener.close()
        with self.assertRaises(ConnectionError):
            listener.accept()

    @unittest.skipIf(os.name != "posix", "RLIMIT_NOFILE is POSIX")
    def test_running_out_of_file_descriptors_makes_accept_raise(self):
        # The real failure: a listener whose accept() hits EMFILE.
        script = """
import os, resource, sys
from rutis import websocket
resource.setrlimit(resource.RLIMIT_NOFILE, (64, 64))
listener = websocket.listen("ws://127.0.0.1:0/rutis", "rutis.2", "secret", announce=lambda a: print(a, flush=True))
held = []
try:
    while True:
        held.append(os.open(os.devnull, os.O_RDONLY))
except OSError:
    pass
print("full", flush=True)
try:
    listener.accept()
except OSError as error:
    print("accept raised:", error, flush=True)
    sys.exit(3)
"""
        import subprocess
        import sys
        from urllib.parse import urlsplit

        package = os.path.dirname(os.path.dirname(os.path.abspath(websocket.__file__)))
        child = subprocess.Popen(
            [sys.executable, "-c", script],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env={**os.environ, "PYTHONPATH": package},
        )
        try:
            address = urlsplit(child.stdout.readline().strip())
            self.assertEqual(child.stdout.readline().strip(), "full")
            # The connection is what makes the child's accept fail; failing,
            # the child closes its listener, which resets the connection
            # queued there. When the reset comes before connect returns,
            # connect raises it (#303): the child still goes on to exit.
            try:
                with socket.create_connection((address.hostname, address.port), timeout=10):
                    code = child.wait(timeout=10)
            except ConnectionResetError:
                code = child.wait(timeout=10)
            out = child.stdout.read()
        finally:
            child.kill()
            child.wait()
            child.stdout.close()
            child.stderr.close()
        self.assertEqual(code, 3, out)
        self.assertIn("accept raised:", out)
        self.assertIn("stopped", out)

    def test_a_listening_runtime_whose_accept_fails_exits_non_zero(self):
        # python -m rutis listen:… ends, with the reason on stderr and exit
        # code 1, instead of a process that cannot be reconnected to.
        script = """
import runpy, sys
from rutis import websocket

class Listener:
    address = "ws://127.0.0.1:1/rutis"
    def accept(self):
        raise OSError("the WebSocket listener stopped: accepting a connection failed")
    def close(self):
        pass

websocket.listen = lambda *args, **kwargs: Listener()
sys.argv = ["rutis", "listen:ws://127.0.0.1:0/rutis", "--id", "py", "--peer", "main", "."]
runpy.run_module("rutis", run_name="__main__")
"""
        import subprocess
        import sys

        package = os.path.dirname(os.path.dirname(os.path.abspath(websocket.__file__)))
        done = subprocess.run(
            [sys.executable, "-c", script],
            capture_output=True,
            text=True,
            timeout=30,
            env={**os.environ, "PYTHONPATH": package},
        )
        self.assertEqual(done.returncode, 1, done.stderr)
        self.assertIn("accepting a connection failed", done.stderr)

    def test_wrong_or_missing_credentials_and_protocols_are_refused(self):
        address, _ = listen()
        cases = [("guess", PROTOCOL, 403), (None, PROTOCOL, 401), ("secret", "rutis.99", 400)]
        for token, protocol, status in cases:
            with self.subTest(token=token, protocol=protocol):
                with self.assertRaises(InvalidStatus) as refused:
                    dial(address, token=token, protocol=protocol)
                self.assertEqual(refused.exception.response.status_code, status)

    def test_a_protocol_refusal_says_which_end_to_upgrade(self):
        # Standard Q6.2.4. guarantee: bridge/version-refusal
        address, _ = listen()
        cases = [
            ("rutis.99", f"this endpoint speaks {PROTOCOL}, the dialer offered rutis.99: upgrade this endpoint (PyPI rutis)"),
            ("rutis.1", f"this endpoint speaks {PROTOCOL}, the dialer offered rutis.1: upgrade the dialer"),
        ]
        for protocol, reason in cases:
            with self.subTest(protocol=protocol):
                with self.assertRaises(InvalidStatus) as refused:
                    dial(address, protocol=protocol)
                self.assertEqual(refused.exception.response.body.decode().strip(), reason)

    def test_a_takeover_closes_with_4002(self):
        address, channels = listen()
        client = dial(address)
        channels.get(timeout=HANG_GUARD).replaced()
        self.assertEqual(closed_code(client), 4002)

    def test_sending_over_the_limit_closes_with_1009(self):
        address, channels = listen()
        client = dial(address)
        channel = channels.get(timeout=HANG_GUARD)
        limit, websocket.MAX_MESSAGE = websocket.MAX_MESSAGE, 64
        try:
            with self.assertRaises(ConnectionError):
                channel.send(b"x" * 65)
        finally:
            websocket.MAX_MESSAGE = limit
        self.assertEqual(closed_code(client), 1009)

    def test_heartbeats_find_a_half_open_connection(self):
        # Timing is what this tests, across a socket: by tolerance
        # (docs/design-test-clocks-2026-10-11.md §3.3), ping and timeout ten
        # times apart, "still connected" after a count of pings rather than
        # a time, no bound on how fast the timeout is found (only a hang
        # guard).
        os.environ["RUTIS_HEARTBEAT"] = "100,1000"
        try:
            address, channels = listen()
        finally:
            del os.environ["RUTIS_HEARTBEAT"]
        relay = FreezableRelay(int(address.rsplit(":", 1)[1].split("/")[0]))
        client = dial(f"ws://127.0.0.1:{relay.port}/rutis", ping_interval=None)
        channel = channels.get(timeout=HANG_GUARD)
        # Alive while pings flow: through more pings and pongs than fit in
        # one timeout (at most 20: this side pings, and is answered, every
        # 100 ms; the client does not ping).
        before = relay.carried
        eventually_blocking("pings through the relay", lambda: relay.carried >= before + 30)
        client.send("{}")
        self.assertEqual(blocking_within("the message", channel.recv), b"{}")
        relay.frozen.set()
        with self.assertRaises(ConnectionError):
            blocking_within("the heartbeat timeout", channel.recv)


def closed_code(client):
    """The close code `client` receives. A connection still open after
    HANG_GUARD fails the test as such, not as an unexpected TimeoutError."""
    try:
        client.recv(timeout=HANG_GUARD)
    except ConnectionClosed as closed:
        return closed.rcvd.code
    except TimeoutError:
        raise AssertionError(f"not closed within {HANG_GUARD} s") from None
    raise AssertionError("a message instead of the close")


# Certificates generated by the Rust tests (see tls/README.md): a CA, and
# certificates it signed for localhost, expired, not valid yet and for
# another name. Their validity periods are fixed dates far from any real
# clock.
TLS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tls")
SECRET = "e6-token-must-not-leak"


def trusting_the_test_ca():
    return ssl.create_default_context(cafile=os.path.join(TLS, "ca.pem"))


def verifying_nothing():
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    return context


def tls_dial(address, context, host):
    """Upgrade to `address` over TLS verified as `host`, presenting the
    token after the TLS handshake; return the TLS socket once the listener
    answered 101. One thread does everything: the websockets threading
    client reads and writes one TLS socket from two threads, which OpenSSL
    does not support and which intermittently loses the handshake response.
    The TCP connection always goes to the listener's 127.0.0.1."""
    port = int(urlsplit(address).port)
    connection = context.wrap_socket(socket.create_connection(("127.0.0.1", port), timeout=HANG_GUARD), server_hostname=host)
    connection.sendall(
        (
            f"GET /rutis HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
            "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n"
            f"Authorization: Bearer {SECRET}\r\nSec-WebSocket-Protocol: {PROTOCOL}\r\n\r\n"
        ).encode()
    )
    response = b""
    while b"\r\n\r\n" not in response:
        chunk = connection.recv(4096)
        if not chunk:
            raise ConnectionError(f"closed after {response!r}")
        response += chunk
    if not response.startswith(b"HTTP/1.1 101"):
        raise ConnectionError(response.decode(errors="replace"))
    return connection


def read_text_frame(connection):
    """One unmasked text frame of under 126 bytes, as the listener sends it."""
    header = connection.recv(2)
    length = header[1] & 0x7F
    payload = b""
    while len(payload) < length:
        payload += connection.recv(length - len(payload))
    return header[0], payload


@unittest.skipIf(websocket is None, "needs the websockets package (rutis[network])")
class TlsRefusalTest(background.TestCase):
    """CPython 3.13.6 deadlocks every wss:// handshake of the websockets
    threading server (#304, python/cpython#137583): the listener refuses to
    start on it with the reason instead of hanging each connection."""

    def test_only_cpython_3_13_6_is_refused(self):
        refusal = websocket.tls_refusal(("cpython", (3, 13, 6)))
        self.assertIn("python/cpython#137583", refusal)
        self.assertIn("3.13.7 or later", refusal)
        for python in (
            ("cpython", (3, 10, 0)),
            ("cpython", (3, 13, 5)),
            ("cpython", (3, 13, 7)),
            ("cpython", (3, 14, 0)),
            ("pypy", (3, 13, 6)),
        ):
            with self.subTest(python=python):
                self.assertIsNone(websocket.tls_refusal(python))

    def test_a_wss_listener_on_the_affected_python_refuses_to_start(self):
        with unittest.mock.patch.object(websocket, "_PYTHON", ("cpython", (3, 13, 6))):
            with self.assertRaises(RuntimeError) as refused:
                websocket.listen(
                    "wss://127.0.0.1:0/rutis",
                    PROTOCOL,
                    SECRET,
                    cert=os.path.join(TLS, "localhost.pem"),
                    key=os.path.join(TLS, "localhost.key"),
                    announce=lambda _: self.fail("announced a listener that cannot serve"),
                )
            self.assertIn("python/cpython#137583", str(refused.exception))
            # ws:// does not use TLS and is not affected.
            listener = websocket.listen("ws://127.0.0.1:0/rutis", PROTOCOL, SECRET, announce=lambda _: None)
            listener.close()

    def test_this_python_serves_wss_or_refuses(self):
        # On CPython 3.13.6 itself, the real interpreter is refused; on any
        # other, WssTest below runs and every handshake has to complete.
        listen = functools.partial(
            websocket.listen,
            "wss://127.0.0.1:0/rutis",
            PROTOCOL,
            SECRET,
            cert=os.path.join(TLS, "localhost.pem"),
            key=os.path.join(TLS, "localhost.key"),
            announce=lambda _: None,
        )
        if websocket.tls_refusal() is not None:
            self.assertRaises(RuntimeError, listen)
        else:
            listen().close()


@unittest.skipIf(websocket is None, "needs the websockets package (rutis[network])")
@unittest.skipIf(
    websocket is not None and websocket.tls_refusal() is not None,
    "this Python cannot serve wss:// (TlsRefusalTest)",
)
class WssTest(background.TestCase):
    """Negative TLS (risk E6). The Python runtime only listens; the dialing
    side verifies the certificate the listener serves. These tests pin that
    the listener serves the configured certificate, that a dialer
    verifying it refuses an untrusted, expired, not yet valid or misnamed
    one before the token is sent, and that a refused handshake delivers no
    channel and leaves the listener accepting."""

    def serve(self, name):
        listener = websocket.listen(
            "wss://127.0.0.1:0/rutis",
            PROTOCOL,
            SECRET,
            cert=os.path.join(TLS, f"{name}.pem"),
            key=os.path.join(TLS, f"{name}.key"),
            announce=lambda _: None,
        )
        self.addCleanup(listener.close)
        return listener

    def assert_delivers(self, listener, connection):
        """The next channel `listener` delivers is `connection`'s."""
        try:
            blocking_within("the accepted connection", listener.accept).send(b'{"to":"this one"}')
            self.assertEqual(read_text_frame(connection), (0x81, b'{"to":"this one"}'))
        finally:
            connection.close()

    def assert_refused(self, listener, host, context, verify_code):
        """Dialing `listener` as `host` with `context` fails the certificate
        check with OpenSSL's `verify_code`, without the token in the error;
        the first channel the listener then delivers is a later connection
        that does not verify."""
        with self.assertRaises(ssl.SSLCertVerificationError) as refused:
            tls_dial(listener.address, context, host)
        self.assertEqual(refused.exception.verify_code, verify_code, refused.exception)
        self.assertNotIn(SECRET, str(refused.exception))
        self.assert_delivers(listener, tls_dial(listener.address, verifying_nothing(), host))

    # risk: E6
    # guarantee: bridge/tls-server-verified
    def test_a_trusted_certificate_for_the_dialed_name_connects(self):
        listener = self.serve("localhost")
        self.assert_delivers(listener, tls_dial(listener.address, trusting_the_test_ca(), "localhost"))

    # risk: E6
    # guarantee: bridge/tls-server-verified
    def test_a_certificate_from_an_untrusted_ca_is_refused(self):
        # The system roots only: the test CA is not among them.
        # 20: X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT_LOCALLY.
        self.assert_refused(self.serve("localhost"), "localhost", ssl.create_default_context(), 20)

    # risk: E6
    # guarantee: bridge/tls-server-verified
    def test_an_expired_or_not_yet_valid_certificate_is_refused(self):
        # 10: X509_V_ERR_CERT_HAS_EXPIRED; 9: X509_V_ERR_CERT_NOT_YET_VALID.
        for name, verify_code in (("expired", 10), ("not-yet-valid", 9)):
            with self.subTest(name):
                self.assert_refused(self.serve(name), "localhost", trusting_the_test_ca(), verify_code)

    # risk: E6
    # guarantee: bridge/tls-server-verified
    def test_a_certificate_for_another_name_is_refused(self):
        # 62: X509_V_ERR_HOSTNAME_MISMATCH; 64: X509_V_ERR_IP_ADDRESS_MISMATCH
        # (a certificate for localhost does not cover 127.0.0.1).
        for name, host, verify_code in (("other-name", "localhost", 62), ("localhost", "127.0.0.1", 64)):
            with self.subTest(name=name, host=host):
                self.assert_refused(self.serve(name), host, trusting_the_test_ca(), verify_code)


class FreezableRelay:
    """A TCP relay that can stop carrying bytes without closing anything. It
    counts what it carried (each read it passed on)."""

    def __init__(self, port):
        listener = socket.create_server(("127.0.0.1", 0))
        self.port = listener.getsockname()[1]
        self.frozen = threading.Event()
        self.carried = 0
        lock = threading.Lock()

        def pump(source, target):
            while data := source.recv(65536):
                if not self.frozen.is_set():
                    target.sendall(data)
                    with lock:
                        self.carried += 1

        def serve():
            client, _ = listener.accept()
            upstream = socket.create_connection(("127.0.0.1", port))
            for source, target in ((client, upstream), (upstream, client)):
                threading.Thread(target=pump, args=(source, target), daemon=True).start()

        threading.Thread(target=serve, daemon=True).start()


if __name__ == "__main__":
    unittest.main()

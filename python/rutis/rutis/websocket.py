"""The WebSocket binding for the Python runtime: it listens, and the
controlling rutis dials (a leaf runtime keeps no reconnection of its own).

One connection carries one channel of UTF-8 JSON text messages; the
session protocol is the subprotocol; the controller presents a bearer token
in the Authorization header, never in the URL. Both sides ping; a far end
silent for 30 s is dropped. Messages are limited to 16 MiB (1009 over it).
Needs the `websockets` package, 15 or newer (its threading server has
heartbeats from 15 on): `pip install rutis[network]`.
"""

from __future__ import annotations

import hmac
import inspect
import os
import queue
import ssl
import sys
import threading
from http import HTTPStatus
from urllib.parse import urlsplit

try:
    from websockets.exceptions import ConnectionClosed, ConnectionClosedOK
    from websockets.sync.server import serve
except ImportError as error:  # pragma: no cover - depends on the environment
    raise ImportError(
        "WebSocket channels need the websockets package: pip install rutis[network]"
    ) from error

MAX_MESSAGE = 16 * 1024 * 1024
GOING_AWAY, TOO_BIG, REPLACED = 1001, 1009, 4002
# Queued after the last connection once nothing accepts any more.
_STOPPED = object()
# The interpreter this runs on; tests stand in another.
_PYTHON = (sys.implementation.name, sys.version_info[:3])


def tls_refusal(python=None) -> str | None:
    """Why this interpreter cannot serve wss://, or None. CPython 3.13.6
    deadlocks a TLS socket that one thread reads (blocked) while another
    writes (python/cpython#137583), and the websockets threading server
    always reads from a thread of its own: every handshake hangs. 3.13.5
    and 3.13.7 or later are not affected, nor is ws://."""
    implementation, version = python or _PYTHON
    if implementation == "cpython" and tuple(version) == (3, 13, 6):
        return (
            "CPython 3.13.6 cannot serve wss:// (its TLS sockets deadlock when one thread "
            "reads while another writes, python/cpython#137583): use 3.13.7 or later"
        )
    return None


def _heartbeat() -> tuple[float, float]:
    """Ping interval and the extra wait for an answer, in seconds. Tests
    shorten them with RUTIS_HEARTBEAT=<ping ms>,<timeout ms>."""
    configured = os.environ.get("RUTIS_HEARTBEAT")
    if configured:
        ping, timeout = (int(part) / 1000 for part in configured.split(","))
        return ping, max(timeout - ping, 0.001)
    return 10.0, 20.0


class WebSocketChannel:
    def __init__(self, connection, ended: threading.Event):
        self._connection = connection
        self._ended = ended

    def send(self, message: bytes) -> None:
        if len(message) > MAX_MESSAGE:
            reason = f"message of {len(message)} bytes exceeds the limit of {MAX_MESSAGE}"
            self.close_with(TOO_BIG, reason)
            raise ConnectionError(reason)
        try:
            self._connection.send(message.decode("utf-8"))
        except ConnectionClosed as error:
            raise ConnectionError(_reason(error)) from error

    def recv(self) -> bytes | None:
        try:
            message = self._connection.recv()
        except ConnectionClosedOK:
            self._ended.set()
            return None
        except ConnectionClosed as error:
            self._ended.set()
            raise ConnectionError(_reason(error)) from error
        if isinstance(message, bytes):
            self.close_with(1003, "binary messages are reserved for a binary encoding")
            raise ConnectionError("received a binary message")
        return message.encode("utf-8")

    def close_with(self, code: int, reason: str) -> None:
        self._connection.close(code, _truncate(reason))
        self._ended.set()

    def close(self, reason: str = "") -> None:
        self.close_with(GOING_AWAY, reason)

    def replaced(self) -> None:
        self.close_with(REPLACED, "replaced by a new connection")


def _truncate(reason: str) -> str:
    encoded = reason.encode("utf-8")[:123]
    return encoded.decode("utf-8", errors="ignore")


def _reason(error: ConnectionClosed) -> str:
    frame = error.rcvd or error.sent
    if frame is None:
        return "connection lost"
    if frame.code == REPLACED:
        return "replaced by a new connection"
    return f"closed ({frame.code}): {frame.reason}"


def protocol_refusal(ours: str, offered: list[str]) -> str:
    """Why a dialer that offered none of this endpoint's session protocol
    is refused, saying which end to upgrade (standard Q6.2.4) when the
    versions it offered (`rutis.<n>`) are all newer (this endpoint) or all
    older (the dialer) than this endpoint's. The wording is rutis-bridge's
    (`transport::websocket::listen::protocol_refusal`)."""

    def version(protocol: str):
        number = protocol[len("rutis."):]
        return int(number) if protocol.startswith("rutis.") and number.isascii() and number.isdigit() else None

    theirs = [protocol for protocol in offered if version(protocol) is not None]
    reason = f"this endpoint speaks {ours}"
    n = version(ours)
    if n is None or not theirs:
        return reason
    reason += f", the dialer offered {', '.join(theirs)}"
    if all(version(protocol) > n for protocol in theirs):
        return f"{reason}: upgrade this endpoint (PyPI rutis)"
    if all(version(protocol) < n for protocol in theirs):
        return f"{reason}: upgrade the dialer"
    return reason


def _print_address(address: str) -> None:
    print(f"rutis: listening on {address}", file=sys.stderr, flush=True)


class Listener:
    """A WebSocket listener for the controlling rutis: every connection
    that presents the token and speaks the protocol is accepted; which one
    is the session is the runtime's decision (a newer one takes over)."""

    def __init__(self, server, accepted: queue.Queue, address: str):
        self._server = server
        self._accepted = accepted
        self.address = address
        self._stopped = threading.Event()
        self._failed: list[BaseException] = []
        self._closing = False
        self._stopping: threading.Thread | None = None
        self._lock = threading.Lock()
        self._serving = threading.Thread(target=self._serve, name="rutis-websocket", daemon=True)
        self._serving.start()

    def _serve(self) -> None:
        try:
            self._server.serve_forever()
            with self._lock:
                closing = self._closing
            if not closing:
                # serve_forever() returns when accept() fails (EMFILE,
                # ENFILE, ENOBUFS...); websockets drops the error itself.
                self._failed.append(
                    OSError("the WebSocket listener stopped: accepting a connection failed (for example, out of file descriptors)")
                )
                self._server.socket.close()
        except BaseException as error:
            # serve_forever() itself returns when accept() fails with an
            # OSError; anything it raises (its selector failing, say) means
            # nothing accepts any more: refuse new connections, and let
            # close() raise what went wrong.
            self._failed.append(error)
            self._server.socket.close()
            raise
        finally:
            self._stopped.set()
            self._accepted.put(_STOPPED)

    def accept(self) -> "WebSocketChannel":
        """Block until the next authenticated connection. Once nothing
        accepts any more (closed, or serving failed), raise at once: what
        stopped the listener, or ConnectionError after close()."""
        channel = self._accepted.get()
        if channel is not _STOPPED:
            return channel
        # A connection that came in after the listener stopped is not left
        # open; the marker goes back, so every later accept() raises too.
        while True:
            try:
                late = self._accepted.get_nowait()
            except queue.Empty:
                break
            if late is not _STOPPED:
                late.close("the listener stopped")
        self._accepted.put(_STOPPED)
        if self._failed:
            raise self._failed[0]
        raise ConnectionError("the WebSocket listener is closed")

    def close(self) -> None:
        """Stop accepting: on return the listening socket is closed and a
        new connection is refused. Established connections stay, with every
        supported websockets version. Raises what stopped the listener, if
        serving or stopping failed; a second call only waits and reports."""

        def stop() -> None:
            try:
                _stop_accepting(self._server)
            except BaseException as error:
                self._failed.append(error)
                self._stopped.set()

        with self._lock:
            first, self._closing = not self._closing, True
        if first and not self._stopped.is_set():
            # websockets 17 waits in shutdown() until every connection ends,
            # so it runs aside; serve_forever() returns once the socket is
            # closed. A failure is raised here, not left to the thread.
            self._stopping = threading.Thread(target=stop, name="rutis-websocket-close", daemon=True)
            self._stopping.start()
        self._stopped.wait()
        if self._failed:
            raise self._failed[0]


def _stop_accepting(server) -> None:
    """Close the listening socket and leave established connections open.
    websockets 17 added `close_connections` to shutdown(), defaulting to
    closing them; before 17, shutdown() only closes the listening socket."""
    if "close_connections" in inspect.signature(server.shutdown).parameters:
        server.shutdown(close_connections=False)
    else:
        server.shutdown()


def listen(
    spec: str,
    protocol: str,
    token: str | None,
    cert: str | None = None,
    key: str | None = None,
    announce=_print_address,
) -> Listener:
    """Listen on `spec` (ws:// on loopback, or wss:// with `cert` and `key`)
    for connections that present `token` and speak `protocol`. `announce`
    gets the bound address (stderr by default)."""
    url = urlsplit(spec)
    secure = url.scheme == "wss"
    host = url.hostname or "127.0.0.1"
    if url.scheme not in ("ws", "wss"):
        raise ValueError(f"{spec} is not a ws:// or wss:// address")
    if not secure and host not in ("127.0.0.1", "::1", "localhost"):
        raise ValueError(f"{spec}: only a loopback listener may go without TLS")
    context = None
    if secure:
        if not (cert and key):
            raise ValueError(f"{spec}: wss needs a certificate and key")
        refusal = tls_refusal()
        if refusal:
            raise RuntimeError(f"{spec}: {refusal}")
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
    path = url.path or "/"

    def check(connection, request):
        if request.path != path:
            return connection.respond(HTTPStatus.NOT_FOUND, "no rutis endpoint here\n")
        header = request.headers.get("Authorization", "")
        if not header.startswith("Bearer "):
            return connection.respond(HTTPStatus.UNAUTHORIZED, "credentials required\n")
        if token is None or not hmac.compare_digest(header[len("Bearer "):].encode(), token.encode()):
            return connection.respond(HTTPStatus.FORBIDDEN, "not accepted here\n")
        offered = [
            item.strip()
            for value in request.headers.get_all("Sec-WebSocket-Protocol")
            for item in value.split(",")
        ]
        if protocol not in offered:
            return connection.respond(HTTPStatus.BAD_REQUEST, protocol_refusal(protocol, offered) + "\n")
        return None

    accepted: queue.Queue = queue.Queue()

    def handler(connection) -> None:
        ended = threading.Event()
        accepted.put(WebSocketChannel(connection, ended))
        # Returning closes the connection: hold it until its session ends.
        ended.wait()

    ping, timeout = _heartbeat()
    server = serve(
        handler,
        host,
        url.port if url.port is not None else (443 if secure else 80),
        ssl=context,
        subprotocols=[protocol],
        process_request=check,
        compression=None,
        max_size=MAX_MESSAGE,
        ping_interval=ping,
        ping_timeout=timeout,
        # A far end that stopped answering will not finish a closing
        # handshake either: do not stretch detection past the timeout.
        close_timeout=min(ping, 1.0),
    )
    bound_host, bound_port = server.socket.getsockname()[:2]
    shown = f"[{bound_host}]" if ":" in bound_host else bound_host
    address = f"{url.scheme}://{shown}:{bound_port}{path}"
    announce(address)
    return Listener(server, accepted, address)


def listen_once(
    spec: str,
    protocol: str,
    token: str | None,
    cert: str | None = None,
    key: str | None = None,
    announce=_print_address,
):
    """The channel of the first connection to `spec` (see [`listen`]); the
    listener then stops accepting."""
    listener = listen(spec, protocol, token, cert, key, announce)
    channel = listener.accept()
    listener.close()
    return channel

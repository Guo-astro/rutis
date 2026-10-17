"""`python3 -m rutis <channel> [--id <endpoint>] [--peer <endpoint>] <project>`:
one runtime process.

The channel is `fd:<n>`, a socket inherited from the process that started
this one; `unix:<path>` (or a bare path), a socket to dial;
`tcp:<host>:<port>`, a loopback address to dial, presenting the token in
RUTIS_CHANNEL_TOKEN first (how processes are started on Windows); or
`listen:ws://…` / `listen:wss://…`, a WebSocket address to listen on for the
controlling rutis (the token it must present in RUTIS_TOKEN, a
listener certificate and key in RUTIS_CERT and RUTIS_KEY); or
`dial:ws://…` / `dial:wss://…`, a rutis access point to dial and to dial
again (`dialing`), presenting the token in RUTIS_TOKEN and trusting the CA
in RUTIS_CA besides the system's.
Local channels speak the compat protocol; network channels the endpoint
format, as `--id`, expecting `--peer` as the controller when given.
"""

import asyncio
import logging
import os
import signal
import socket
import sys
import threading
import traceback

from .peer import Peer
from .runner import Runtime


TOKEN = "RUTIS_CHANNEL_TOKEN"


def open_channel(spec: str):
    if spec.startswith("fd:"):
        return socket.socket(fileno=int(spec[len("fd:"):]))
    if spec.startswith("tcp:"):
        host, _, port = spec[len("tcp:"):].rpartition(":")
        # Spent once connected: what this process starts does not inherit it.
        token = os.environ.pop(TOKEN, None)
        if not token:
            raise ValueError(f"{TOKEN} is not set for {spec}")
        connection = socket.create_connection((host.strip("[]"), int(port)))
        connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        connection.sendall(token.encode() + b"\n")
        return connection
    path = spec[len("unix:"):] if spec.startswith("unix:") else spec
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    connection.connect(path)
    return connection


def parse(argv: list[str]) -> tuple[str, dict | None, str]:
    """The channel, the endpoint (network channels), and the project."""
    if not argv:
        raise ValueError("usage: python3 -m rutis <channel> [--id <endpoint>] [--peer <endpoint>] <project>")
    channel, rest, flags = argv[0], list(argv[1:]), {}
    while rest and rest[0].startswith("--"):
        flag = rest.pop(0)[2:]
        if not rest:
            raise ValueError(f"--{flag} needs a value")
        flags[flag] = rest.pop(0)
    if len(rest) != 1:
        raise ValueError("usage: python3 -m rutis <channel> [--id <endpoint>] [--peer <endpoint>] <project>")
    endpoint = None
    if channel.startswith(("listen:", "dial:")):
        if "id" not in flags:
            raise ValueError(f"a network channel needs --id <endpoint>: {channel}")
        # A runner is a runtime: the controller manages its rows.
        endpoint = {"local": flags["id"], "expected": flags.get("peer"), "declare": ["runtime"]}
    if channel.startswith("dial:"):
        # What retrying cannot fix fails here, before any dial.
        from . import websocket
        from .peer import _ENDPOINT_ID

        websocket.dial_host(channel[len("dial:"):])
        if not _ENDPOINT_ID.match(flags["id"]):
            raise ValueError(f"invalid endpoint id {flags['id']}: use lower-case letters, digits and -")
    return channel, endpoint, rest[0]


class Session:
    """One control session and the lease it holds: the rows, proxies and
    references of one controller."""

    def __init__(self, channel, endpoint: dict | None):
        self.channel = channel
        self.runtime = Runtime()
        self.peer = Peer(channel, self.runtime.dispatch, settled=None, endpoint=endpoint)
        self.runtime.peer = self.peer

    def start(self) -> None:
        self.peer.start()

    async def end(self, replaced: bool = False) -> None:
        """End the lease: no frame of this session is read any more, then
        every row and proxy goes."""
        if replaced and hasattr(self.channel, "replaced"):
            self.channel.replaced()
        self.peer.close(ConnectionError("replaced by a new connection" if replaced else "session ended"))
        self.runtime.closing = True
        await self.runtime.dispose()

    async def stop(self) -> None:
        """Stop for good: every row goes while the controller still hears
        from this end, then the connection closes. Cleanups that do not
        finish within the deadline are given up, as when a session ends."""
        give_up_after(shutdown_deadline())
        self.runtime.closing = True
        await self.runtime.dispose()
        self.peer.close(ConnectionError("the runtime stopped"))
        self.channel.close("the runtime stopped")


async def run(channel: str, endpoint: dict | None, project: str) -> int:
    if project and project not in sys.path:
        sys.path.insert(0, project)
    if channel.startswith("listen:"):
        await serve(channel[len("listen:"):], endpoint)
        return 0
    if channel.startswith("dial:"):
        return await dial_out(channel[len("dial:"):], endpoint)
    session = Session(open_channel(channel), endpoint)
    session.start()
    await session.peer.closed
    # The rows go now, unless the host disposed them first: it went away
    # (it was killed, say) and nothing else ends this process. Cleanups that
    # do not finish within the deadline are given up (exit status 1), also
    # one that blocks the event loop.
    give_up_after(shutdown_deadline())
    await session.end()
    return 0


DEADLINE = "RUTIS_SHUTDOWN_TIMEOUT"


def shutdown_deadline() -> float:
    """How long cleanups may take once the session ended, in seconds:
    RUTIS_SHUTDOWN_TIMEOUT (rutis-host passes its own deadline on), 10 by
    default."""
    try:
        seconds = float(os.environ.get(DEADLINE, ""))
    except ValueError:
        return 10.0
    return seconds if 0 < seconds < float("inf") else 10.0


def give_up_after(seconds: float) -> None:
    """End the process in `seconds` unless it ended before (main exits it
    once the cleanups ran)."""

    def give_up() -> None:
        try:
            print(
                f"rutis: cleanups did not finish within {seconds:g} s after the session ended; exiting",
                file=sys.stderr,
                flush=True,
            )
        finally:
            # Also when the line cannot be written: stderr is a pipe to a
            # host that was killed, and the write fails (BrokenPipeError).
            os._exit(1)

    timer = threading.Timer(seconds, give_up)
    timer.daemon = True
    timer.start()


async def serve(spec: str, endpoint: dict) -> None:
    """A runtime that stays up and listens: it serves one controller at a
    time, a newer connection taking over, and cleans each lease up when it
    ends. A new session is greeted only once the old lease is gone. It
    stops on SIGTERM and Ctrl-C: the rows go while the controller still
    hears from this end, the connection and the listener close."""
    from . import websocket
    from .dialing import report
    from .peer import ENDPOINT_PROTOCOL

    listener = await asyncio.to_thread(
        websocket.listen,
        spec,
        f"rutis.{ENDPOINT_PROTOCOL}",
        os.environ.get("RUTIS_TOKEN"),
        os.environ.get("RUTIS_CERT"),
        os.environ.get("RUTIS_KEY"),
    )
    stopping = asyncio.Event()
    stop_on_signals(stopping)
    stopped = asyncio.ensure_future(stopping.wait())
    current: Session | None = None
    accepting = asyncio.ensure_future(asyncio.to_thread(listener.accept))
    while True:
        waiting = {accepting, stopped} | ({current.peer.closed} if current else set())
        done, _ = await asyncio.wait(waiting, return_when=asyncio.FIRST_COMPLETED)
        if stopped in done:
            if current is not None:
                await current.stop()
            # Closing the listener ends the thread blocked in accept(),
            # which the event loop waits for before the process exits.
            await asyncio.to_thread(listener.close)
            try:
                late = await accepting
            except (ConnectionError, OSError):
                return
            late.close("the runtime stopped")
            return
        if accepting in done:
            try:
                channel = accepting.result()
            except BaseException:
                # Nothing accepts any more: a runtime nobody can reconnect
                # to ends, so its process manager sees it and restarts it.
                if current is not None:
                    await current.end()
                raise
            accepting = asyncio.ensure_future(asyncio.to_thread(listener.accept))
            if current is not None:
                report("session ended (replaced by a new connection)")
                await current.end(replaced=True)
            current = Session(channel, endpoint)
            current.start()
        elif current is not None and current.peer.closed in done:
            report(f"session ended ({ended_because(current)}); waiting for a controller")
            await current.end()
            current = None


def ended_because(session: Session) -> str:
    """Why a session ended, for its state line: what ended the connection
    (closed by the controller, a heartbeat timeout, lost), else what ended
    the session."""
    ended_by = getattr(session.channel, "ended_by", None)
    if ended_by:
        return ended_by
    reason = str(session.peer.closed_error or "the session closed")
    return reason.removeprefix("session failed: ")


async def dial_out(spec: str, endpoint: dict) -> int:
    """A runtime that dials a rutis access point and stays connected: each
    connection is a new session, whose rows go when it ends; it dials again
    by the rules of `dialing` until it is stopped (SIGTERM, Ctrl-C: the rows
    go, then the connection closes) or the access point is incompatible
    (exit status 1)."""
    from . import websocket
    from .dialing import keep_dialing
    from .peer import ENDPOINT_PROTOCOL

    # Spent once read: what this process starts does not inherit it.
    token = os.environ.pop("RUTIS_TOKEN", None)
    ca = os.environ.get("RUTIS_CA")
    try:
        tls = websocket.client_tls(ca) if spec.startswith("wss:") else None
    except (OSError, ValueError) as error:
        # ssl.SSLError is an OSError: a file that holds no certificate.
        print(f"rutis: RUTIS_CA {ca}: {error}", file=sys.stderr, flush=True)
        return 1
    protocol = f"rutis.{ENDPOINT_PROTOCOL}"
    stopping = asyncio.Event()
    stop_on_signals(stopping)

    def start(channel) -> Session:
        session = Session(channel, endpoint)
        session.start()
        return session

    return await keep_dialing(
        f"{spec} as {endpoint['local']}",
        lambda: websocket.dial(spec, protocol, token, tls),
        start,
        stopping,
    )


def stop_on_signals(stopping: asyncio.Event) -> None:
    """Set `stopping` on SIGINT and SIGTERM (where the platform has it)."""
    loop = asyncio.get_running_loop()
    for name in ("SIGINT", "SIGTERM"):
        number = getattr(signal, name, None)
        if number is None:
            continue
        try:
            loop.add_signal_handler(number, stopping.set)
        except NotImplementedError:
            # Windows: no loop signal handlers.
            signal.signal(number, lambda *_: loop.call_soon_threadsafe(stopping.set))


def main() -> None:
    # The state lines are UTF-8 everywhere: on Windows a pipe would get the
    # ANSI code page, and an OS error's text in it.
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="backslashreplace")
    try:
        channel, endpoint, project = parse(sys.argv[1:])
    except (ValueError, ImportError) as error:
        sys.exit(str(error))
    code = 0
    try:
        code = asyncio.run(run(channel, endpoint, project))
    except Exception:  # noqa: BLE001 - reported; the exit code says it failed
        traceback.print_exc()
        code = 1
    finally:
        # Stray plugin threads must not keep the process alive: rutis waits
        # for it to exit.
        sys.stdout.flush()
        sys.stderr.flush()
        logging.shutdown()
        os._exit(code)


main()

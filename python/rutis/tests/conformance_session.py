"""The session conformance target, served by the Python session: connects
to the channel given as the first argument (a Unix socket path, or
`tcp:<host>:<port>`, presenting the token in RUTIS_CHANNEL_TOKEN first, or
`listen:ws://…`, taking the first connection that presents RUTIS_TOKEN) as
endpoint `python`, expecting `main`, and serves `conformance` until the
session ends."""

import asyncio
import os
import socket
import sys

from rutis.peer import Peer


class Conformance:
    def __init__(self) -> None:
        self.held = None
        self.started = False
        self.aborted = False
        self.kept = {}
        self.peer: Peer | None = None

    def echo(self, value):
        return value

    def apply(self, fn, value):
        return fn(value)

    async def later(self, value):
        await asyncio.sleep(0.005)
        return value

    def fail(self, name, message):
        error = type(name, (Exception,), {})(message)
        raise error

    def hold(self, fn):
        self.held = fn

    def fire(self, value):
        if self.held is None:
            raise LookupError("nothing held")
        return self.held(value)

    def drop(self):
        if self.held is not None:
            self.peer.release(self.held)
        self.held = None

    async def abortable(self, signal):
        self.started = True
        try:
            await signal.wait()
        finally:
            # Cancelled: the signal is set, and the task is cancelled too.
            self.aborted = signal.cancelled

    def is_aborted(self):
        return self.aborted

    def is_started(self):
        return self.started

    def reenter(self, fn):
        return fn()

    # The release-count property test (risk P4).
    def keep(self, slot, fn, *_copies):
        self.kept[slot] = fn

    def free(self, slot):
        fn = self.kept.pop(slot, None)
        if fn is not None:
            self.peer.release(fn)

    def make(self):
        return lambda value: value

    # The value one level deeper, returned or passed to a callback: past the
    # depth limit, this side's call or reply fails (#279).
    def wrap(self, value):
        return [value]

    def apply_wrapped(self, fn, value):
        return fn([value])

    def tables(self):
        return self.peer.references()

    # What a call cannot send (#675): a result or an argument over the
    # 16 MiB limit fails that call on this side, and an object with
    # methods does not go to a far end without `objects`.
    def big(self, size):
        return "x" * size

    def apply_big(self, fn, size):
        return fn("x" * size)

    def make_object(self):
        return Counter()


class Counter:
    """An object with methods: it crosses by reference, only to a far end
    that declared `objects`."""

    def __init__(self) -> None:
        self.n = 0

    def inc(self):
        self.n += 1
        return self.n


def dial(spec: str):
    if spec.startswith("listen:"):
        from rutis.peer import ENDPOINT_PROTOCOL
        from rutis.websocket import listen_once

        protocol = f"rutis.{ENDPOINT_PROTOCOL}"
        return listen_once(spec[len("listen:"):], protocol, os.environ["RUTIS_TOKEN"])
    if spec.startswith("tcp:"):
        host, _, port = spec[len("tcp:"):].rpartition(":")
        connection = socket.create_connection((host.strip("[]"), int(port)))
        connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        connection.sendall(os.environ["RUTIS_CHANNEL_TOKEN"].encode() + b"\n")
        return connection
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    connection.connect(spec)
    return connection


async def main(spec: str) -> None:
    connection = await asyncio.to_thread(dial, spec)
    service = Conformance()

    def dispatch(target, method, args):
        if target != "conformance":
            raise LookupError(f"no target {target}")
        name = {
            "aborted": "is_aborted",
            "started": "is_started",
            "applyWrapped": "apply_wrapped",
            "applyBig": "apply_big",
            "object": "make_object",
        }.get(method, method)
        operation = getattr(service, name, None)
        if operation is None or name.startswith("_"):
            raise LookupError(f"no method {method}")
        return operation(*args)

    peer = Peer(connection, dispatch, endpoint={"local": "python", "expected": "main"})
    service.peer = peer
    peer.start()
    await peer.closed


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1]))

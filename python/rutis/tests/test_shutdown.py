"""A runtime whose host goes away: it unloads its rows, runs their
cleanups, and exits within its shutdown deadline (RUTIS_SHUTDOWN_TIMEOUT,
#353, #682, #985) — also when a cleanup never finishes (exit status 1 at
the deadline), and when a plugin keeps its event loop busy.

A real runtime process (`python -m rutis`) on a Unix socket, the host a
script here: closing this end is the host going away, as when it was
killed. The waits here are the tests' hang guard (support.HANG_GUARD),
never the deadline they measure: a runtime that does not exit fails the
test at the guard, saying what it was doing, instead of hanging (#985)."""

# guarantee: host/killed-host-leaves-no-runtimes — a runtime ends within its
# deadline once its host went away, and a deadline that cannot be armed ends
# the process at once rather than never.

import json
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

import rutis
from rutis.peer import ENDPOINT_PROTOCOL
from support import HANG_GUARD, Hung, blocking_within, eventually_blocking

# Work that would keep the process going, and a cleanup that leaves a mark
# and, with `hang`, never returns.
LINGERING = """
import asyncio
import threading
import time


def apply(ctx, config):
    loop = asyncio.get_running_loop()
    loop.call_later(3600, lambda: None)
    threading.Thread(target=lambda: time.sleep(3600)).start()

    async def cleanup():
        with open(config["marker"], "w") as marker:
            marker.write("cleaned up")
        if config.get("hang"):
            await asyncio.Event().wait()

    return cleanup
"""

# Keeps the event loop for ever once it starts: the runtime cannot unload
# its rows, or see the session end, on that thread any more.
BUSY = """
import asyncio


def apply(ctx, config):
    def spin():
        with open(config["marker"], "w") as marker:
            marker.write("busy")
        while True:
            pass

    asyncio.get_running_loop().call_later(0.05, spin)
"""

PACKAGE = os.path.dirname(os.path.dirname(os.path.abspath(rutis.__file__)))


class Session:
    """One runtime process and this side of its session: a Unix socket it
    connects to and is greeted on, and whose end this side closes when the
    host goes away."""

    def __init__(self, test, plugins, deadline):
        self.directory = tempfile.TemporaryDirectory()
        test.addCleanup(self.directory.cleanup)
        for name, source in plugins.items():
            with open(os.path.join(self.directory.name, f"{name}.py"), "w") as module:
                module.write(source)
        path = os.path.join(self.directory.name, "host.sock")
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        test.addCleanup(self.server.close)
        self.server.bind(path)
        self.server.listen(1)
        self.server.settimeout(HANG_GUARD)
        project = self.directory.name
        self.child = subprocess.Popen(
            [sys.executable, "-m", "rutis", path, "--id", "runtime", "--peer", "host", project],
            cwd=project,
            env={
                **os.environ,
                "PYTHONPATH": os.pathsep.join([PACKAGE, project]),
                "PYTHONUNBUFFERED": "1",
                "RUTIS_SHUTDOWN_TIMEOUT": deadline,
            },
            stdin=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        test.addCleanup(self._ended)
        self.calls = 0
        self.connection, _ = self.server.accept()
        self.connection.settimeout(HANG_GUARD)
        self.reader = self.connection.makefile("rb")
        self.connection.sendall(
            json.dumps(
                {
                    "op": "hello",
                    "version": ENDPOINT_PROTOCOL,
                    "endpoint": "host",
                    "implementation": {"name": "rutis-host", "version": "0.19.3"},
                    "capabilities": [],
                }
            ).encode()
            + b"\n"
        )
        greeting = self._read()
        assert greeting["op"] == "hello", greeting

    def _read(self) -> dict:
        line = self.reader.readline()
        if not line:
            raise EOFError("the runtime closed the session")
        return json.loads(line)

    def load(self, key, module, config) -> None:
        """Load the plugin `module` as the row `key`, as rutis does."""
        self.calls += 1
        call = f"host:{self.calls}"
        self.connection.sendall(
            json.dumps(
                {
                    "op": "invoke",
                    "id": call,
                    "path": [],
                    "target": "",
                    "method": "rows.load",
                    "args": {"type": "data", "value": [key, module, config, [], [], [], {"scope": []}]},
                }
            ).encode()
            + b"\n"
        )
        reply = self._read()
        assert reply["id"] == call and reply["op"] == "return", reply

    def host_goes_away(self) -> None:
        """Its end of the channel closes, with no `dispose` first: what a
        host that was killed leaves behind."""
        self.reader.close()
        self.connection.close()
        self.gone = time.monotonic()

    def wait(self) -> int:
        """How it exited, within the hang guard; failing, it says what the
        process was doing instead of leaving the test hanging."""
        try:
            return blocking_within("the runtime exits once its host went away", self.child.wait)
        except Hung as hung:
            self.child.kill()
            self.child.wait()
            raise Hung(f"{hung}; its process {self.child.pid} was still running: {self.diagnostics()}") from None

    def diagnostics(self) -> str:
        """What it wrote to its own stderr, once it ended."""
        try:
            return self.child.stderr.read()
        finally:
            self.child.stderr.close()

    def _ended(self) -> None:
        if self.child.poll() is None:
            self.child.kill()
            self.child.wait()
        if not self.child.stderr.closed:
            self.child.stderr.close()


def session(test, plugin, deadline, config=None, name="lingering") -> Session:
    """A runtime that loaded `plugin` as the row `name`, which left nothing
    but the session."""
    session = Session(test, {name: plugin}, deadline)
    session.load(name, name, dict({"marker": os.path.join(session.directory.name, "marker")}, **(config or {})))
    return session


class Deadline(unittest.TestCase):
    """The runtime ends within its deadline once its host went away."""

    @unittest.skipUnless(hasattr(socket, "AF_UNIX"), "Unix sockets")
    def test_it_exits_once_its_cleanups_ran(self):
        # An hour: the runtime exits once its cleanups did, not at its
        # deadline, though the plugin's timer and thread would keep it.
        it = session(self, LINGERING, "3600")
        it.host_goes_away()
        code = it.wait()
        self.assertEqual(code, 0, it.diagnostics())
        self.assertEqual(
            pathlib.Path(it.directory.name, "marker").read_text(), "cleaned up", "the row's cleanup ran before it exited"
        )
        self.assertLess(time.monotonic() - it.gone, HANG_GUARD, "and not at its deadline")

    @unittest.skipUnless(hasattr(socket, "AF_UNIX"), "Unix sockets")
    def test_it_gives_up_a_cleanup_at_its_deadline(self):
        it = session(self, LINGERING, "0.5", {"hang": True})
        it.host_goes_away()
        code = it.wait()
        took = time.monotonic() - it.gone
        stderr = it.diagnostics()
        self.assertEqual(code, 1, stderr)
        self.assertGreaterEqual(took, 0.5, f"it exited after {took} s, before its cleanup's deadline")
        self.assertEqual(
            stderr,
            "rutis: cleanups did not finish within 0.5 s after the session ended "
            "(or a plugin keeps the runtime busy); exiting\n",
        )

    @unittest.skipUnless(hasattr(socket, "AF_UNIX"), "Unix sockets")
    def test_a_plugin_keeping_the_loop_busy_does_not_keep_it_alive(self):
        # The busy plugin holds the event loop when the host goes away: the
        # thread reading the channel arms the deadline, which ends it.
        it = session(self, BUSY, "1", name="busy")
        marker = os.path.join(it.directory.name, "marker")
        eventually_blocking("the plugin to keep its runtime busy", lambda: os.path.exists(marker))
        it.host_goes_away()
        code = it.wait()
        took = time.monotonic() - it.gone
        self.assertEqual(code, 1, it.diagnostics())
        self.assertGreaterEqual(took, 1, f"it exited after {took} s, before its deadline")


class Arming(unittest.TestCase):
    """The deadline cannot be lost: a timer that does not start ends the
    process now rather than never."""

    def setUp(self):
        from rutis import __main__ as main

        self.main = main
        main._given_up = False
        self.addCleanup(setattr, main, "_given_up", False)

    def test_it_ends_the_process_when_its_timer_cannot_start(self):
        with mock.patch.object(threading.Timer, "start", side_effect=RuntimeError("can't start new thread")):
            with mock.patch.object(self.main.os, "_exit") as exited:
                with mock.patch.object(self.main, "say") as said:
                    self.main.give_up_after(10)
        exited.assert_called_once_with(1)
        self.assertEqual(
            said.call_args.args[0],
            "rutis: the deadline that would end this process is not running "
            "(its timer cannot start: can't start new thread); exiting",
        )


if __name__ == "__main__":
    unittest.main()

"""Test a plugin on a real rutis host::

    from rutis.testing import load

    async def test_weather():
        async with load(weather_plugin, config={"city": "Oslo"}, services={"llm": FakeLlm()}) as t:
            assert await t.service("weather").today() == "sunny in Oslo"

`load` starts `rutis-host test` (the file `RUTIS_HOST` names, else
`rutis-host` on PATH) and connects this process's Python runtime to it, in
the test's event loop. The plugin is a row of that host, run by the
runtime as every row is: the host's loader checks its config and fills in
the defaults, every event goes through the host, and unloading is the
host's. The services the test gives (`services`) and the test's own
listeners (`on`) belong to a plugin of the test, `tester`, in the same
runtime. Plugins loaded with one `Bus` share its host; one loaded without
gets a host of its own.

A failing `emit` listener is reported by the host (printed, and in
`await t.reported()`), never raised.
"""

from __future__ import annotations

import asyncio
import inspect
import itertools
import os
import shutil
import socket
import subprocess
import sys
from contextlib import asynccontextmanager
from types import ModuleType
from typing import Any, AsyncIterator, Callable, NamedTuple

from . import plugin as sdk
from .events import Emitter
from .peer import Peer, host_gone
from .runner import Runtime, no_method

# The test's own plugin, as the host knows it: the row that provides the
# test's services and holds its listeners.
TESTER = "tester"
# This runtime's endpoint on the session with `rutis-host test`.
ENDPOINT = {"local": "python", "declare": ["runtime"]}
# How long `rutis-host test` is given to start, and to exit once its
# session ended, in seconds.
STARTING = 30.0
STOPPING = 10.0


class PluginTestError(AssertionError):
    """The plugin cannot be tested as asked: the host refused to load it,
    it does not provide a service, or `rutis-host` cannot be started."""


class Reported(NamedTuple):
    """A failure nobody receives (§5.6), as the host reported it: a listener
    of the row failing in an `emit`, or a filter of its `emit`. `line`: the
    host's line (`ticker: listener of tick failed: ValueError: boom`, then
    the stack)."""

    line: str

    def __str__(self) -> str:
        return self.line


def _said(error: BaseException) -> str:
    """What the host said: the message of its error."""
    return getattr(error, "message", None) or str(error)


def _host_path() -> str:
    path = os.environ.get("RUTIS_HOST") or shutil.which("rutis-host")
    if not path:
        raise PluginTestError(
            "rutis.testing runs plugins on rutis-host: install the rutis-host package "
            "(a dev dependency of the plugin) or set RUTIS_HOST to its file"
        )
    return path


# The host each runtime of this process belongs to, for `tester`.
_HOSTS: dict[int, "_Host"] = {}
# The names plugins are registered under for the runtime to find them.
_ENTRIES = itertools.count(1)


def _tester(ctx: Any, config: Any) -> None:
    # `ctx` is the runtime's context of the row: its runtime says whose.
    host = _HOSTS.get(id(ctx._runtime))
    if host is not None and not host.started.done():
        host.started.set_result(ctx)


sys.modules[TESTER] = module = ModuleType(TESTER)
module.plugin = sdk.define_plugin(_tester, inject=["rutis-host/test"], name=TESTER)
del module


class _Host:
    """`rutis-host test`, and this process's runtime on its session."""

    def __init__(self, bus: "Bus") -> None:
        self.bus = bus
        self.runtime: Runtime | None = None
        self.process: subprocess.Popen | None = None
        self.socket: socket.socket | None = None
        self.peer: Peer | None = None
        self.tester: Any = None
        self.harness: Any = None

    async def start(self) -> None:
        loop = asyncio.get_running_loop()
        path = _host_path()
        ours, theirs = socket.socketpair()
        try:
            self.process = subprocess.Popen([path, "test", TESTER], stdin=theirs, stdout=theirs)
        except OSError as error:
            ours.close()
            raise PluginTestError(f"cannot start {path}: {error}") from None
        finally:
            theirs.close()
        self.socket = ours
        self.started = loop.create_future()
        self.runtime = Runtime()
        _HOSTS[id(self.runtime)] = self
        self.peer = Peer(ours, self.runtime.dispatch, settled=None, endpoint=dict(ENDPOINT))
        self.runtime.peer = self.peer
        self.runtime.traced(self.peer)
        self.peer.start()
        exited = asyncio.ensure_future(asyncio.to_thread(self.process.wait))
        try:
            done, _ = await asyncio.wait({self.started, exited}, timeout=STARTING, return_when=asyncio.FIRST_COMPLETED)
            if self.started not in done:
                status = f"exited with status {exited.result()}" if exited in done else f"did not start within {STARTING:g} s"
                raise PluginTestError(f"rutis-host test {status}")
        except BaseException:
            await self.stop()
            raise
        finally:
            if not exited.done():
                exited.cancel()
        self.tester = self.started.result()
        self.harness = self.tester.use("rutis-host/test")

    def row(self, id: str) -> Any:
        """The runtime's row the host calls `id`, while it is loaded here."""
        for key, named in self.runtime.bus.row_ids.items():
            if named == id and key in self.runtime.rows:
                return self.runtime.rows[key]
        return None

    @property
    def tester_row(self) -> Any:
        return self.tester._row

    async def stop(self) -> None:
        """End the session: the runtime's rows go, then `rutis-host test`
        sees the session end and exits."""
        runtime, self.runtime = self.runtime, None
        if runtime is not None:
            runtime.closing = True
            try:
                await runtime.dispose()
            finally:
                _HOSTS.pop(id(runtime), None)
        if self.peer is not None:
            self.peer.close(host_gone("the test ended", "stopped"))
        if self.socket is not None:
            try:
                self.socket.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            self.socket.close()
        if self.process is not None:
            try:
                await asyncio.to_thread(self.process.wait, STOPPING)
            except subprocess.TimeoutExpired:
                self.process.kill()
                await asyncio.to_thread(self.process.wait)


class Bus:
    """One host for the plugins loaded with it: they hear each other's
    events. A plugin loaded without one gets a host of its own. The host
    starts with the first `load` and stops when the last one ends."""

    def __init__(self) -> None:
        self._host: _Host | None = None
        # The rows loaded with it, in order.
        self._rows: list[str] = []
        self._loaded = 0
        self._starting: asyncio.Future | None = None

    async def reported(self) -> list[Reported]:
        """What the host reported of the rows loaded with this bus, row by
        row, while its host runs."""
        if self._host is None:
            raise PluginTestError("nothing is loaded with this bus")
        return [Reported(line) for row in self._rows for line in await self._host.harness.reported(row)]

    async def _enter(self) -> _Host:
        self._loaded += 1
        try:
            if self._starting is None:
                host = _Host(self)
                self._starting = asyncio.ensure_future(host.start())
                await self._starting
                self._host = host
            else:
                await asyncio.shield(self._starting)
        except BaseException:
            await self._leave()
            raise
        return self._host

    async def _leave(self) -> None:
        self._loaded -= 1
        if self._loaded:
            return
        host, self._host, self._starting = self._host, None, None
        self._rows.clear()
        if host is not None:
            await host.stop()

    def emitter(self) -> Emitter:
        """Emit as the test: `emit`, `parallel`, `serial`, `waterfall`,
        `bail` and `filter`, while a plugin is loaded with this bus."""
        if self._host is None:
            raise PluginTestError("nothing is loaded with this bus")
        host = self._host
        return Emitter(lambda *request: host.runtime.send(host.tester_row, *request))


class Loaded:
    """A plugin loaded on a host: its row `row`."""

    def __init__(self, host: _Host, row: str, entry: str, plugin: sdk.Plugin) -> None:
        self._host = host
        self._entry = entry
        self._plugin = plugin
        self._unloaded = False
        self.bus = host.bus
        self.row = row
        # The test's own listeners (`on`), withdrawn when it unloads.
        self._listening: list[Callable[[], None]] = []
        self._emitter = Emitter(self._send)

    def _loaded_row(self) -> Any:
        row = self._host.row(self.row) if not self._unloaded else None
        if row is None:
            raise PluginTestError("the plugin is unloaded" if self._unloaded else f"row {self.row} is not running")
        return row

    def _send(self, *request: Any) -> Any:
        return self._host.runtime.send(self._loaded_row(), *request)

    # ── Events, as the plugin's row emits them ───────────────────

    def emit(self, name: str, *args: Any) -> None:
        """Emit from the plugin's row: every listener starts, a failure is
        reported (`reported`), never raised."""
        self._emitter.emit(name, *args)

    def parallel(self, name: str, *args: Any):
        """`await t.parallel(name, *args)`: AggregateError if any failed."""
        return self._emitter.parallel(name, *args)

    def serial(self, name: str, *args: Any):
        """`await t.serial(name, *args)`: the first result, or None."""
        return self._emitter.serial(name, *args)

    def waterfall(self, name: str, *args: Any, terminal: Callable | None = None):
        """`await t.waterfall(name, *args, terminal=...)`: the outermost
        listener's return value."""
        return self._emitter.waterfall(name, *args, terminal=terminal)

    def bail(self, name: str, *args: Any) -> Any:
        """The first result, synchronously; AsyncListener on an
        asynchronous listener."""
        return self._emitter.bail(name, *args)

    def filter(self, predicate: Callable[[dict], Any]) -> Emitter:
        """The five methods, delivering only where `predicate(descriptor)`."""
        return self._emitter.filter(predicate)

    async def reported(self) -> list[Reported]:
        """What the host reported of this row: each listener of it that
        failed in an `emit`, each filter of its `emit` that failed."""
        return [Reported(line) for line in await self._host.harness.reported(self.row)]

    def on(self, name: str, listener: Callable, *, prepend: bool = False, global_: bool = False) -> Callable[[], None]:
        """Listen to the event `name` from the test's plugin until the
        returned function is called or the plugin unloads. `listener` may be
        `async`; `prepend` and `global_` as `ctx.on`."""
        if self._unloaded:
            raise PluginTestError("the plugin is unloaded")
        off = self._host.tester.on(name, listener, prepend=prepend, global_=global_)

        def stop() -> None:
            off()
            if stop in self._listening:
                self._listening.remove(stop)

        self._listening.append(stop)
        return stop

    # ── Services ─────────────────────────────────────────────────

    def service(self, name: str) -> Any:
        """The service `name` the plugin provides, called as the host calls
        it: its declared methods, or, undeclared, the object's public
        methods, all async."""
        runtime = self._host.runtime
        row = self._loaded_row()
        id = row.id(name)
        slot = runtime.slots.get(id)
        if id not in row.provided or slot is None or slot.handle is None:
            raise PluginTestError(f"the plugin does not provide {name} (now)")
        handle = slot.handle
        shape = self.methods(name)

        class Service:
            def __getattr__(self, method: str) -> Any:
                # A method it does not have, as the runtime says it.
                if method.startswith("__"):
                    raise AttributeError(method)
                raise no_method(AttributeError, name, method, shape)

            def __repr__(self) -> str:
                return f"<service {name}>"

        seen = Service()
        for method, kind in shape.items():
            if kind == "sync":
                def call(*args: Any, _method: str = method) -> Any:
                    return runtime.dispatch(handle, _method, list(args))
            else:
                async def call(*args: Any, _method: str = method) -> Any:
                    value = runtime.dispatch(handle, _method, list(args))
                    while inspect.isawaitable(value):
                        value = await value
                    return value

            call.__name__ = method
            call.__qualname__ = f"{name}.{method}"
            setattr(seen, method, call)
        return seen

    def methods(self, name: str) -> dict:
        """The methods the host sees on the service `name`: `{ method:
        "sync" | "async" }`."""
        row = self._loaded_row()
        provided = self._host.runtime.services.get(row.id(name))
        if provided is None:
            raise PluginTestError(f"the plugin does not provide {name} (now)")
        shape = self._plugin.provides.get(name)
        if shape is None:
            shape = {method: "async" for method in sorted(sdk.methods_of(provided[1]))}
        return dict(shape)

    def provided(self) -> list[str]:
        """The names of the services the plugin provides now."""
        return [id.split("\0", 1)[0] for id in self._loaded_row().provided]

    async def update(self, config: Any) -> None:
        """Change the row's config, as the host does: the row unloads and
        applies again with `config`; a config its schema refuses leaves it
        as it was."""
        if self._unloaded:
            raise PluginTestError("the plugin is unloaded")
        try:
            await self._host.harness.update(self.row, config)
        except Exception as error:  # noqa: BLE001 - the host's words
            raise PluginTestError(_said(error)) from None

    async def unload(self) -> None:
        """Unload as the host does: the plugin's listeners hear nothing more
        and its services are withdrawn, its running listeners are cancelled
        and waited for, then its cleanups run, latest first."""
        if self._unloaded:
            return
        self._unloaded = True
        try:
            await self._host.harness.unload(self.row)
        except Exception as error:  # noqa: BLE001 - the host's words
            raise PluginTestError(_said(error)) from None
        finally:
            sys.modules.pop(self._entry, None)
            # After the row: what it emitted before it went still reaches them.
            for stop in list(self._listening):
                stop()


@asynccontextmanager
async def load(
    plugin: sdk.Plugin | ModuleType,
    *,
    config: Any = None,
    services: dict | None = None,
    bus: Bus | None = None,
    row: str | None = None,
) -> AsyncIterator[Loaded]:
    """Load `plugin` (a module with `apply`, or what `define_plugin` returns)
    with `config` and the services in `services` as a row of a host;
    unload it on exit.

    `bus`: share the host with the other plugins loaded with it (default:
    a host of its own). `row`: its row's id (default the plugin's declared
    name, else its module's); the host adds `-2`, `-3`… to an id taken."""
    module_name = plugin.__name__ if isinstance(plugin, ModuleType) else None
    if isinstance(plugin, ModuleType):
        plugin = sdk.load(plugin)
    if not isinstance(plugin, sdk.Plugin):
        raise PluginTestError("load needs a plugin module or what define_plugin returns")
    name = row or plugin.name or (module_name.rsplit(".", 1)[-1] if module_name else None) or "plugin"
    if not name or any(c in name for c in "/\\."):
        raise PluginTestError(f"a row id has no '/', '\\' or '.': {name!r}")
    bus = bus if bus is not None else Bus()
    if not isinstance(bus, Bus):
        raise PluginTestError("bus must be a rutis.testing.Bus")
    host = await bus._enter()
    withdraws: list[Callable[[], None]] = []
    # The runtime finds the plugin under this name; the host calls its row
    # after the last part.
    entry = f"rutis-testing/{next(_ENTRIES)}/{name}"
    entered = None
    try:
        for given, service in (services or {}).items():
            withdraws.append(host.tester.provide(given, service))
        sys.modules[entry] = holder = ModuleType(entry)
        holder.plugin = plugin
        try:
            id = await host.harness.load(entry, config)
        except Exception as error:  # noqa: BLE001 - the host's words
            sys.modules.pop(entry, None)
            raise PluginTestError(_said(error)) from None
        entered = Loaded(host, id, entry, plugin)
        bus._rows.append(id)
        if host.row(id) is None:
            missing = [n for n in plugin.inject if n not in host.runtime.services and n not in host.runtime.hosts]
            raise PluginTestError(
                f"{id} did not start: it injects {', '.join(missing) or 'a service'}, "
                "which nothing provides: give the test services="
            )
        yield entered
    finally:
        try:
            if entered is not None:
                await entered.unload()
        finally:
            for withdraw in reversed(withdraws):
                withdraw()
            await bus._leave()


__all__ = ["Bus", "Loaded", "PluginTestError", "Reported", "load"]

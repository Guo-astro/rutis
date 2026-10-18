"""Test a plugin without a host: give it the services it injects, call the
services it provides, unload it::

    from rutis.testing import load

    async def test_weather():
        async with load(weather_plugin, config={"city": "Oslo"}, services={"llm": FakeLlm()}) as t:
            assert await t.service("weather").today() == "sunny in Oslo"

It checks what a host would: the plugin uses only the services it declares
in `inject`, provides what it declares in `provides` with every declared
method, and runs its cleanups on unload. A service `provides` does not
declare is used as rutis uses it: its methods are the object's public
callables, all async. With `strict` (the default), values
cross between the plugin and the test as they would between processes: data
is copied (a dataclass arrives as a dict; a datetime, Decimal, UUID, set,
bytes or an int beyond ±(2^53−1) as the runtime sends and receives it,
#308), functions and objects with
behaviour pass by reference, sync methods return values and async ones
awaitables; what works only in one process fails here too.

Events work as on the runtime, all in the test's process (always its fast
path): the five ways to emit, `filter`, scopes, and unload waiting for
running listeners and cancelling them. The loaded plugin's `emit`,
`parallel`, `serial`, `waterfall`, `bail` and `filter` emit as its row
does (values crossing with `strict`). The plugin's own `ctx` emitters pass
values as they are, as the runtime's fast path does, but with `strict` the
errors reaching them arrive as from another process (`RemoteError`, matched
by `name`). Descriptors give `runtime` "python". Plugins loaded with one
`Bus` hear each other, each in its scope::

    bus = Bus(scopes={"t1": "tenant"})
    async with load(audit, bus=bus) as root, load(chat, bus=bus, scope="t1") as tenant:
        assert await tenant.serial("chat/check", "hi") == "ok"
        tenant.emit("chat/said", "hi")   # a failing listener: in bus.reported

A failing `emit` listener is reported (printed, and in `reported`), never
raised, as on the runtime.
"""

from __future__ import annotations

import dataclasses
import inspect
import math
from contextlib import asynccontextmanager
from types import ModuleType
from typing import Any, AsyncIterator, Callable, NamedTuple

from . import events
from . import plugin as sdk
from .events import Emitter, failed_line, reserved
from .runner import shutdown_deadline
from . import _values
from .peer import (
    MAX_DEPTH,
    UNDEFINED,
    AggregateError,
    BindingError,
    RemoteError,
    _beyond_data,
    _is_live,
    decode_error,
    encode_error,
)


class PluginTestError(AssertionError):
    """The plugin does something a host would refuse or that would break
    across processes."""


def _cross(value: Any, where: str, data: bool = False, depth: int = 0) -> Any:
    """`value` as the other process sees it, by the rules of the runtime
    (`rutis.peer`); `data`: inside plain data, which crosses as JSON;
    `depth`: the lists and dicts around it (at most MAX_DEPTH, #279)."""
    if value is None or isinstance(value, (bool, str)):
        return value
    if value is UNDEFINED:
        return None
    tag = _values.tag_of(value)
    if tag is not None:
        return _cross_tagged(tag, value, where, depth)
    if isinstance(value, int):
        # Within ±(2^53−1): beyond it, an int is a bigint (above).
        return value
    if isinstance(value, float):
        if not math.isfinite(value):
            raise PluginTestError(f"{where}: {value} cannot cross between processes (only finite numbers do)")
        return value
    if isinstance(value, (list, tuple, dict, BaseException)) and depth >= MAX_DEPTH:
        raise PluginTestError(
            f"{where}: a value nested deeper than {MAX_DEPTH} levels cannot cross between processes"
        )
    if isinstance(value, (list, tuple)):
        data = data or not _beyond_data(value)
        return [_cross(item, f"{where}[{i}]", data, depth + 1) for i, item in enumerate(value)]
    if isinstance(value, dict):
        data = data or not _beyond_data(value)
        return {key: _cross(item, f"{where}.{key}", data, depth + 1) for key, item in value.items()}
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        try:
            fields = dataclasses.asdict(value)
        except RecursionError:
            raise PluginTestError(
                f"{where}: a value nested deeper than {MAX_DEPTH} levels cannot cross between processes"
            ) from None
        return _cross(fields, where, True, depth)
    # An exception passed as a value crosses as data: its name and message.
    if isinstance(value, BaseException) and not data:
        return {"name": type(value).__name__, "message": str(value)}
    if inspect.isawaitable(value):
        return _awaited(value, where)
    if callable(value) and not isinstance(value, type):
        return _function(value, where)
    if _is_live(value):
        return _Reference(value, where)
    raise PluginTestError(f"{where}: {type(value).__name__} cannot cross between processes")


def _cross_tagged(tag: str, value: Any, where: str, depth: int) -> Any:
    """A tagged value (#308) as the other process receives it: written and
    read as the runtime does (an instant in UTC, to the microsecond; a map
    as a dict subclass that crosses as a map again; bytes as bytes; an
    int beyond ±(2^53−1) as one that crosses as a bigint again), refused
    where the runtime refuses."""
    try:
        if tag not in ("map", "set"):
            return _values.read(tag, _values.write(tag, value))
        if depth >= MAX_DEPTH:
            raise PluginTestError(
                f"{where}: a value nested deeper than {MAX_DEPTH} levels cannot cross between processes"
            )
        if tag == "set":
            elements = list(value)
            _values.check_keys(elements, "set element")
            return _values.received_set(
                [_cross(element, f"{where} element", False, depth + 1) for element in elements]
            )
        entries = list(value.items())
        _values.check_keys((key for key, _item in entries), "map key")
        return _values.received_map(
            [_cross(key, f"{where} key", False, depth + 1) for key, _item in entries],
            [_cross(item, f"{where}[{key!r}]", False, depth + 1) for key, item in entries],
        )
    except (ValueError, _values.Refused) as error:
        raise PluginTestError(f"{where}: {error}") from None


def _crossed_error(error: Exception) -> Exception:
    """An exception as the other process receives it: a `RemoteError` with
    its name, message, `code` and `data` (rutis's own named errors as their
    classes; an AggregateError with its children crossed, in order); its
    class and other attributes stay behind. The testing tool's own errors
    are not crossed."""
    if isinstance(error, PluginTestError):
        return error
    return decode_error(encode_error(error))


async def _awaited(value: Any, where: str) -> Any:
    try:
        result = await value
    except Exception as error:
        raise _crossed_error(error) from error
    return _cross(result, where)


def _function(function: Callable, where: str) -> Callable:
    def call(*args: Any) -> Any:
        crossed = [_cross(arg, f"{where} argument {i}") for i, arg in enumerate(args)]
        try:
            result = function(*crossed)
        except Exception as error:
            raise _crossed_error(error) from error
        return _cross(result, where)

    return call


class _Reference:
    """An object with behaviour, used from the other process."""

    def __init__(self, target: Any, where: str) -> None:
        object.__setattr__(self, "_target", target)
        object.__setattr__(self, "_where", where)

    def __getattr__(self, name: str) -> Any:
        return _cross(getattr(self._target, name), f"{self._where}.{name}")


def _shaped(name: str, service: Any, shape: dict, strict: bool) -> Any:
    """A service seen through its declared shape: only declared methods, sync
    ones returning values, async ones awaitables."""

    class Service:
        pass

    seen = Service()
    for method, kind in shape.items():
        def call(*args: Any, _method: str = method, _kind: str = kind) -> Any:
            where = f"{name}.{_method}"
            crossed = [_cross(arg, f"{where} argument {i}") for i, arg in enumerate(args)] if strict else list(args)
            try:
                result = getattr(service, _method)(*crossed)
            except Exception as error:
                if not strict:
                    raise
                raise _crossed_error(error) from error
            if _kind == "sync":
                if inspect.isawaitable(result):
                    if inspect.iscoroutine(result):
                        result.close()
                    raise PluginTestError(f"{where} is declared sync but returned an awaitable: declare it 'async'")
                return _cross(result, where) if strict else result

            async def settle() -> Any:
                try:
                    value = await result if inspect.isawaitable(result) else result
                except Exception as error:
                    if not strict:
                        raise
                    raise _crossed_error(error) from error
                return _cross(value, where) if strict else value

            return settle()

        setattr(seen, method, call)
    return seen


class Reported(NamedTuple):
    """A failure nobody receives (§5.6): a listener failing in an `emit`
    (`kind` "listener", `row` its row), or the filter of an `emit`
    (`kind` "filter", `row` the emitter's)."""

    kind: str
    name: str
    row: str | None
    error: BaseException

    def __str__(self) -> str:
        return failed_line(self.kind, self.name, self.row, self.error)


class Bus:
    """The event bus of one process, for tests. Plugins loaded with the
    same `bus` hear each other, each in its scope; one loaded without gets
    a bus of its own.

    `scopes` names the group of each scope path (`{"t1": "tenant",
    "t1/s1": "session"}`); a path not named is a group of its last part.
    Everything runs in the test's process, as the fast path of the
    runtime. The tool does not emit `rutis/*` events."""

    def __init__(self, *, scopes: dict | None = None) -> None:
        self.scopes = dict(scopes or {})
        # The failures nobody receives, in order: each is also printed, as
        # the runtime prints it.
        self.reported: list[Reported] = []
        self._bus = events.Bus(report=self._report, runtime="python")
        self._rows: set = set()

    def _report(self, kind: str, name: str, row: str | None, error: BaseException) -> None:
        self.reported.append(Reported(kind, name, row, error))
        events.print_report(kind, name, row, error)

    def chain(self, path: str | None) -> list:
        """The scope chain of a scope path: `root` (or None, or "") is the
        root, `t1/s1` the session `t1/s1` inside the tenant `t1`."""
        if path in (None, "", "root"):
            return []
        if not isinstance(path, str):
            raise PluginTestError(f"a scope is a path such as t1/s1, not {path!r}")
        parts = path.split("/")
        if not all(parts):
            raise PluginTestError(f"invalid scope path {path!r}")
        return [
            {"id": "/".join(parts[: i + 1]), "group": self.scopes.get("/".join(parts[: i + 1]), part)}
            for i, part in enumerate(parts)
        ]

    def listeners(self, name: str | None = None) -> list[dict]:
        """What a filter sees of each listener (of `name`), in order."""
        return [
            entry.descriptor(self._bus.runtime)
            for entry in list(self._bus.entries)
            if name is None or entry.name == name
        ]

    def emitter(self, scope: str | None = "root", *, row: str = "test", strict: bool = True) -> Emitter:
        """Emit as a row `row` at `scope` would: `emit`, `parallel`,
        `serial`, `waterfall`, `bail` and `filter`. With `strict`, the
        arguments and what comes back cross as between processes."""
        return Emitter(_send(self._bus, row, self.chain(scope), strict))


def _send(bus: events.Bus, row: str, scope: list, strict: bool) -> Callable:
    """How the test emits: as the row `row` at `scope`, its values crossing
    as between processes with `strict`."""

    def send(mode: str, name: str, args: list, terminal, keep) -> Any:
        if strict:
            args = [_cross(arg, f"event {name} argument {i}") for i, arg in enumerate(args)]
        if mode == "emit":
            bus.dispatch(mode, name, scope, args, keep, terminal, row)
            return None
        if mode == "bail":
            try:
                value = bus.dispatch(mode, name, scope, args, keep, terminal, row)
            except Exception as error:
                if not strict:
                    raise
                raise _crossed_error(error) from error
            return _cross(value, f"{mode} of {name}") if strict else value
        dispatched = bus.dispatch(mode, name, scope, args, keep, terminal, row)
        if not strict:
            return dispatched
        return _awaited(dispatched, f"{mode} of {name}")

    return send


class Loaded:
    def __init__(
        self,
        plugin: sdk.Plugin,
        strict: bool,
        bus: Bus,
        row: str,
        scope: list,
        name: str,
        runtime: str | None = "python",
        deadline: float | None = None,
    ) -> None:
        self._plugin = plugin
        self._strict = strict
        self._provided: dict[str, list] = {}
        self._cleanups: list[Callable] = []
        self._unloaded = False
        self._ctx: Any = None
        self.bus = bus
        self.row = row
        self.scope = scope
        self.name = name
        # What its listeners' descriptors give as `runtime`.
        self.runtime = runtime
        self._deadline = deadline
        self._emitter = Emitter(_send(bus._bus, row, scope, strict))

    # ── Events, as the plugin's row emits them ───────────────────

    def emit(self, name: str, *args: Any) -> None:
        """Emit from the plugin's row and scope: every listener starts, a
        failure is reported (`reported`), never raised."""
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

    @property
    def reported(self) -> list[Reported]:
        """The failures nobody received on this plugin's bus."""
        return self.bus.reported

    # ── Services ─────────────────────────────────────────────────

    def service(self, name: str) -> Any:
        """The service `name` the plugin provides, as rutis sees it: its
        declared methods, or, undeclared, the object's public methods, all
        async."""
        if self._unloaded:
            raise PluginTestError("the plugin is unloaded")
        return _shaped(name, self._provided_object(name), self.methods(name), self._strict)

    def methods(self, name: str) -> dict:
        """The methods rutis sees on the service `name`: `{ method: "sync" |
        "async" }`."""
        value = self._provided_object(name)
        shape = self._plugin.provides.get(name)
        if shape is None:
            shape = {method: "async" for method in sorted(sdk.methods_of(value))}
        return dict(shape)

    def _provided_object(self, name: str) -> Any:
        if name not in self._provided:
            raise PluginTestError(f"the plugin does not provide {name} (now)")
        return self._provided[name][0]

    def provided(self) -> list[str]:
        """The names of the services the plugin provides now."""
        return list(self._provided)

    async def _apply(self, config: Any) -> None:
        # Run the plugin's apply with `config` on this row.
        config = config if config is not None else {}
        returned = self._plugin.apply(self._ctx, _cross(config, "config") if self._strict else config)
        if inspect.isawaitable(returned):
            returned = await returned
        if returned is not None:
            if not callable(returned):
                raise PluginTestError("apply must return a cleanup function or None")
            self._cleanups.append(returned)

    async def update(self, config: Any) -> None:
        """Change the row's config, as a host's `rows.update`: the row
        unloads (as `unload`) and applies again with `config` under the same
        row key, so its listeners register again, after every existing
        one."""
        if self._unloaded:
            raise PluginTestError("the plugin is unloaded")
        await self.unload()
        self._unloaded = False
        self.bus._rows.add(self.row)
        await self._apply(config)

    async def unload(self) -> None:
        """Unload as the runtime does: the plugin's listeners hear nothing
        more and its services are withdrawn, and its emitters refuse
        (BindingError); its running listeners are cancelled and waited for
        (at most `deadline`, or RUTIS_SHUTDOWN_TIMEOUT, then a line says
        so); then its cleanups run, latest first."""
        if self._unloaded:
            return
        self._unloaded = True
        bus = self.bus._bus
        bus.withdraw_row(self.row)
        self._provided.clear()
        try:
            await bus.stop(self.row, shutdown_deadline() if self._deadline is None else self._deadline)
            cleanups = self._cleanups[::-1]
            self._cleanups.clear()
            for cleanup in cleanups:
                result = cleanup()
                if inspect.isawaitable(result):
                    await result
        finally:
            self.bus._rows.discard(self.row)


class _Context(sdk.Context):
    def __init__(self, loaded: Loaded, services: dict) -> None:
        self._loaded = loaded
        self._services = services
        # Between plugins of one process values do not cross, as on the
        # runtime's fast path; errors do, as from another process.
        self._emitter = Emitter(_own(loaded))

    def use(self, name: str) -> Any:
        plugin = self._loaded._plugin
        if name not in plugin.inject:
            raise PluginTestError(f"the plugin uses {name} without declaring it in inject")
        service = self._services[name]
        return _cross(service, f"service {name}") if self._loaded._strict else service

    def provide(self, name: str, value: Any) -> Callable[[], None]:
        shape = self._loaded._plugin.provides.get(name)
        for method in shape or {}:
            if not callable(getattr(value, method, None)):
                raise PluginTestError(f"{name} is declared with {method} in provides, but the service has no such method")
        entry = [value]
        self._loaded._provided[name] = entry

        def withdraw() -> None:
            if self._loaded._provided.get(name) is entry:
                del self._loaded._provided[name]

        return withdraw

    def effect(self, cleanup: Callable) -> None:
        if not callable(cleanup):
            raise PluginTestError("effect needs a cleanup function")
        self._loaded._cleanups.append(cleanup)

    def on(self, name: str, listener: Callable, *, prepend: bool = False, global_: bool = False) -> Callable[[], None]:
        loaded = self._loaded
        if loaded._unloaded:
            raise PluginTestError("the plugin is unloaded")
        reserved(name, listening=True)
        bus = loaded.bus._bus
        plugin = loaded._plugin.name or loaded.row
        entry = bus.add(name, loaded.row, plugin, listener, loaded.scope, prepend, global_)
        entry.runtime = loaded.runtime

        def off() -> None:
            bus.withdraw(entry)

        return off

    def emit(self, name: str, *args: Any) -> None:
        self._emitter.emit(name, *args)

    def parallel(self, name: str, *args: Any):
        return self._emitter.parallel(name, *args)

    def serial(self, name: str, *args: Any):
        return self._emitter.serial(name, *args)

    def waterfall(self, name: str, *args: Any, terminal: Callable | None = None):
        return self._emitter.waterfall(name, *args, terminal=terminal)

    def bail(self, name: str, *args: Any) -> Any:
        return self._emitter.bail(name, *args)

    def filter(self, predicate: Callable[[dict], Any]) -> Emitter:
        return self._emitter.filter(predicate)


def _own(loaded: Loaded) -> Callable:
    """How the loaded plugin's row emits: refused once it unloaded; with
    `strict`, the errors reaching it arrive as from another process (a
    `RemoteError` matched by `name`, an AggregateError's children too),
    since a listener may be in one."""
    bus = loaded.bus._bus

    def send(mode: str, name: str, args: list, terminal, keep) -> Any:
        if loaded._unloaded:
            raise BindingError(f"row {loaded.row} is not loaded in this runtime")
        if mode == "emit":
            bus.dispatch(mode, name, loaded.scope, args, keep, terminal, loaded.row)
            return None
        try:
            dispatched = bus.dispatch(mode, name, loaded.scope, args, keep, terminal, loaded.row)
        except Exception as error:
            if not loaded._strict:
                raise
            raise _crossed_error(error) from error
        if mode == "bail" or not loaded._strict:
            return dispatched
        return _errors_crossed(dispatched)

    return send


async def _errors_crossed(awaitable: Any) -> Any:
    try:
        return await awaitable
    except Exception as error:
        raise _crossed_error(error) from error


@asynccontextmanager
async def load(
    plugin: sdk.Plugin | ModuleType,
    *,
    config: Any = None,
    services: dict | None = None,
    strict: bool = True,
    bus: Bus | None = None,
    scope: str | None = "root",
    row: str | None = None,
    emits: Any = None,
    runtime: str | None = "python",
    deadline: float | None = None,
) -> AsyncIterator[Loaded]:
    """Load `plugin` (a module with `apply`, or what `define_plugin` returns)
    with `config` and the services in `services`; unload it on exit.

    `bus`: share events with the other plugins loaded with it (default: a
    bus of its own). `scope`: the scope path it is loaded in (`t1/s1`;
    default the root). `row`: its row key and id on the bus (default the
    plugin's declared name, else its module's); its listeners' descriptors
    give the declared name as `plugin`, else the row. `runtime`: what they
    give as `runtime` (default "python"). `deadline`: how long unload
    waits for running listeners, in seconds (default
    RUTIS_SHUTDOWN_TIMEOUT, 10)."""
    if emits is not None:
        raise PluginTestError(
            "the load option emits was removed in rutis 0.13: listeners register themselves; "
            "emit with t.emit / parallel / serial / waterfall / bail (docs/migration-0.13.md)"
        )
    name = plugin.__name__ if isinstance(plugin, ModuleType) else None
    if isinstance(plugin, ModuleType):
        plugin = sdk.load(plugin)
    if not isinstance(plugin, sdk.Plugin):
        raise PluginTestError("load needs a plugin module or what define_plugin returns")
    if plugin.api > sdk.PLUGIN_API:
        raise PluginTestError(f"the plugin needs plugin API {plugin.api}; this SDK supports {sdk.PLUGIN_API}")
    name = plugin.name or name or getattr(plugin.apply, "__module__", None) or "plugin"
    services = services or {}
    for injected in plugin.inject:
        if injected not in services:
            raise PluginTestError(f"the plugin injects {injected}: give the test a service {injected}")
    bus = bus if bus is not None else Bus()
    if not isinstance(bus, Bus):
        raise PluginTestError("bus must be a rutis.testing.Bus")
    row = row or name
    if row in bus._rows:
        raise PluginTestError(f"row {row} is already loaded on this bus: give it another row=")
    if deadline is not None and not (isinstance(deadline, (int, float)) and 0 <= deadline < math.inf):
        raise PluginTestError(f"deadline is a number of seconds, not {deadline!r}")
    loaded = Loaded(plugin, strict, bus, row, bus.chain(scope), name, runtime, deadline)
    bus._rows.add(row)
    ctx = _Context(loaded, services)
    loaded._ctx = ctx
    try:
        await loaded._apply(config)
        yield loaded
    finally:
        await loaded.unload()


__all__ = ["Bus", "Loaded", "PluginTestError", "Reported", "load"]

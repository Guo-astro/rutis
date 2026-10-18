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

Unlike the runtime, an uncaught async listener from `ctx.emit` does not
exit the test process: unload cancels unfinished listeners and reports
already completed failures as an AggregateError.
"""

from __future__ import annotations

import dataclasses
import inspect
import math
from contextlib import asynccontextmanager
from types import ModuleType
from typing import Any, AsyncIterator, Callable

from . import plugin as sdk
from .events import Events, refused_emits
from . import _values
from .peer import (
    MAX_DEPTH,
    UNDEFINED,
    AggregateError,
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
    its name and message; its class and other attributes stay behind. The
    testing tool's own errors are not crossed."""
    if isinstance(error, PluginTestError):
        return error
    if isinstance(error, (AggregateError, RemoteError)):
        return decode_error(encode_error(error))
    return RemoteError(type(error).__name__, str(error))


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


class Loaded:
    def __init__(self, plugin: sdk.Plugin, strict: bool, emits=()) -> None:
        self._plugin = plugin
        self._strict = strict
        self._provided: dict[str, list] = {}
        self._cleanups: list[Callable] = []
        self._disposers: list[Callable] = []
        self._unloaded = False
        self._events = Events()
        # As the Node test tool: a string is not a list of one name.
        if not isinstance(emits, (list, tuple)) or not all(isinstance(name, str) for name in emits):
            raise PluginTestError("emits must be a list of event names")
        # As the runners' mount: internal/* cannot be declared for emission.
        refused = refused_emits(emits)
        if refused is not None:
            raise PluginTestError(refused)
        self._emits = set(emits)

    async def emit(self, name: str, *args: Any) -> None:
        """Emit a declared host event, waiting for every listener to settle."""
        if not isinstance(name, str):
            raise TypeError(f"an event name is a string, not {type(name).__name__}")
        if name.startswith("internal/"):
            raise PluginTestError(f"event {name} is internal to the runtime")
        if self._unloaded:
            raise PluginTestError("the plugin is unloaded")
        if name not in self._emits:
            raise PluginTestError(f"event {name} is not declared for emission from rutis")
        values = [_cross(arg, f"event {name} argument {i}") for i, arg in enumerate(args)] if self._strict else args
        try:
            await self._events.parallel(name, *values)
        except Exception as error:
            if not self._strict:
                raise
            raise decode_error(encode_error(error)) from error

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

    async def unload(self, *, _report_events=True) -> None:
        """Run the cleanups, latest first; the plugin's services are withdrawn."""
        if self._unloaded:
            return
        self._unloaded = True
        for off in self._disposers[:]:
            off()
        cleanups = self._cleanups[::-1]
        self._cleanups.clear()
        cleaned = False
        try:
            for cleanup in cleanups:
                result = cleanup()
                if inspect.isawaitable(result):
                    await result
            cleaned = True
        finally:
            self._events.clear()
            self._provided.clear()
            await self._events.drain(report=_report_events and cleaned)


class _Context(sdk.Context):
    def __init__(self, loaded: Loaded, services: dict) -> None:
        self._loaded = loaded
        self._services = services

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

    def on(self, name: str, listener: Callable) -> Callable[[], None]:
        if self._loaded._unloaded:
            raise PluginTestError("the plugin is unloaded")
        return self._loaded._events.on(name, listener, self._loaded._disposers)

    def emit(self, name: str, *args: Any) -> None:
        self._loaded._events.emit(name, *args)


@asynccontextmanager
async def load(
    plugin: sdk.Plugin | ModuleType,
    *,
    config: Any = None,
    services: dict | None = None,
    strict: bool = True,
    emits=(),
) -> AsyncIterator[Loaded]:
    """Load `plugin` (a module with `apply`, or what `define_plugin` returns)
    with `config` and the services in `services`; unload it on exit."""
    if isinstance(plugin, ModuleType):
        plugin = sdk.load(plugin)
    if not isinstance(plugin, sdk.Plugin):
        raise PluginTestError("load needs a plugin module or what define_plugin returns")
    if plugin.api > sdk.PLUGIN_API:
        raise PluginTestError(f"the plugin needs plugin API {plugin.api}; this SDK supports {sdk.PLUGIN_API}")
    services = services or {}
    for name in plugin.inject:
        if name not in services:
            raise PluginTestError(f"the plugin injects {name}: give the test a service {name}")
    loaded = Loaded(plugin, strict, emits)
    ctx = _Context(loaded, services)
    config = config if config is not None else {}
    completed = False
    try:
        returned = plugin.apply(ctx, _cross(config, "config") if strict else config)
        if inspect.isawaitable(returned):
            returned = await returned
        if returned is not None:
            if not callable(returned):
                raise PluginTestError("apply must return a cleanup function or None")
            loaded._cleanups.append(returned)
        yield loaded
        completed = True
    finally:
        await loaded.unload(_report_events=completed)


__all__ = ["Loaded", "PluginTestError", "load"]

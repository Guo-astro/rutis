"""What a Python plugin for rutis looks like.

A plugin is a module with an `apply(ctx, config)` function. It may declare:

- `inject`: the names of the services it uses. rutis starts the plugin only
  once all of them are available, and stops it when one goes away.
- `provides` (optional): the services it provides to rutis, as
  `{ name: { method: "sync" | "async" } }`, or `{ name: SomeClass }` to
  take the methods from the class (public methods; `async def` ones are
  async). Without it, what `ctx.provide` provides still reaches rutis: its
  methods are the object's public callables, all async. With it, methods
  can be `sync`, and `rutis-host check` knows before the plugin runs.
- `Config`: the JSON Schema of its config, as a dict.

`apply` may be `async`. It returns a cleanup function (which may be async)
or None::

    inject = ["llm"]

    def apply(ctx, config):
        ctx.provide("weather", Weather(ctx.use("llm"), config["city"]))
        return lambda: None

A module may instead define `plugin = rutis.define_plugin(...)`.

A packaged plugin registers its module under the entry point group
`rutis.plugins`; a host names it `py:<entry point name>`.
"""

from __future__ import annotations

import inspect
from dataclasses import dataclass, field
from types import ModuleType
from typing import Any, Awaitable, Callable


class Context:
    """What `apply` receives."""

    def use(self, name: str) -> Any:
        """The service `name`: the object itself when a plugin in this
        process provides it, otherwise a proxy whose methods call it."""
        raise NotImplementedError

    def provide(self, name: str, value: Any) -> Callable[[], None]:
        """Provide `value` as the service `name` until the plugin unloads,
        or until the returned function is called."""
        raise NotImplementedError

    def effect(self, cleanup: Callable) -> None:
        """Run `cleanup` (sync or async) when the plugin unloads."""
        raise NotImplementedError

    def on(self, name: str, listener: Callable, *, prepend: bool = False, global_: bool = False) -> Callable[[], None]:
        """Listen to the event `name` until the returned function is called
        or the plugin unloads; wherever the event is emitted (another
        plugin, another runtime, the host, a linked node), it reaches
        `listener(*args)` when the emitter's scope is this plugin's or
        inside it. `listener` may be `async def`; in a waterfall it gets
        `next` as its last argument. Its return value is what `serial`,
        `bail` and `waterfall` see.

        `prepend`: before every listener registered so far. `global_`:
        exempt from emitters' filters (never from the scope rule).
        `rutis/*` events (published by rutis) can be listened to;
        `internal/*` cannot.

        On unload, a running listener's task is cancelled and waited for,
        at most RUTIS_SHUTDOWN_TIMEOUT."""
        raise NotImplementedError

    def emit(self, name: str, *args: Any) -> None:
        """Start every listener of `name` and return at once. A listener's
        failure is reported where it runs, never raised here."""
        raise NotImplementedError

    def parallel(self, name: str, *args: Any) -> Awaitable[None]:
        """`await ctx.parallel(name, *args)`: run every listener, wait for
        all; failures raise `rutis.AggregateError` in registration order."""
        raise NotImplementedError

    def serial(self, name: str, *args: Any) -> Awaitable[Any]:
        """`await ctx.serial(name, *args)`: run the listeners one at a time
        and return the first result (anything but None and False), or None.
        An error stops and is raised."""
        raise NotImplementedError

    def waterfall(self, name: str, *args: Any, terminal: Callable | None = None) -> Awaitable[Any]:
        """`await ctx.waterfall(name, *args, terminal=...)`: the outermost
        listener's return value, as is. Each listener gets `next` last;
        `await next()` runs the rest of the chain and finally
        `terminal(*args)`. Calling `next` twice raises WaterfallNext."""
        raise NotImplementedError

    def bail(self, name: str, *args: Any) -> Any:
        """`serial`, synchronously. An asynchronous listener fails it with
        `rutis.AsyncListener`."""
        raise NotImplementedError

    def filter(self, predicate: Callable[[dict], Any]) -> Any:
        """An emitter with the same five methods that delivers only to the
        listeners `predicate(descriptor)` keeps. A descriptor is a dict:
        name, prefix, plugin, row, runtime, node, scope, global."""
        raise NotImplementedError


# The plugin API this SDK writes plugins against. A runtime refuses a plugin
# that needs a newer one, naming both. 3: parallel, serial, waterfall, bail
# and filter (rutis 0.13); runtimes still load plugins of API 2.
PLUGIN_API = 3


@dataclass
class Plugin:
    apply: Callable
    inject: list = field(default_factory=list)
    provides: dict = field(default_factory=dict)
    config: dict | None = None
    # The plugin API it was written against.
    api: int = PLUGIN_API
    # What it provides beyond `provides` reaches rutis by itself, known once
    # it runs: true of every plugin of this SDK.
    announces: bool = True


def define_plugin(
    apply: Callable,
    *,
    inject: list | tuple = (),
    provides: dict | None = None,
    config: dict | None = None,
) -> Plugin:
    return Plugin(apply, list(inject), shapes(provides or {}), config)


def shapes(provides: dict) -> dict:
    """`{ name: { method: kind } }` from declared shapes or classes."""
    result = {}
    for name, declared in provides.items():
        if isinstance(declared, dict):
            for method, kind in declared.items():
                if kind not in ("sync", "async"):
                    raise ValueError(f"{name}.{method}: kind must be 'sync' or 'async'")
            result[name] = dict(declared)
        elif isinstance(declared, type):
            result[name] = {
                method: "async" if inspect.iscoroutinefunction(member) else "sync"
                for method, member in inspect.getmembers(declared, callable)
                if not method.startswith("_")
            }
        else:
            raise TypeError(f"provides[{name!r}] must be a dict of method kinds or a class")
    return result


def methods_of(value: Any) -> set:
    """The methods of a service no declaration describes: its public
    callable attributes (not starting with `_`), its class's and its own;
    a property is not read."""
    methods = set()
    for name in dir(value):
        if name.startswith("_"):
            continue
        try:
            static = inspect.getattr_static(value, name)
        except AttributeError:
            continue
        if isinstance(static, property):
            continue
        if callable(getattr(value, name, None)):
            methods.add(name)
    return methods


def load(module: ModuleType) -> Plugin:
    declared = getattr(module, "plugin", None)
    if isinstance(declared, Plugin):
        return declared
    apply = getattr(module, "apply", None)
    if not callable(apply):
        raise TypeError(f"{module.__name__} has no apply(ctx, config)")
    inject = getattr(module, "inject", [])
    if isinstance(inject, str) or not all(isinstance(name, str) for name in inject):
        raise TypeError(f"{module.__name__}.inject must be a list of service names")
    return Plugin(
        apply,
        list(inject),
        shapes(getattr(module, "provides", {})),
        getattr(module, "Config", None),
    )

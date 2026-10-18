"""The Python runtime: leaf plugins loaded one by one for rutis-loader.

rutis owns the plugin model: dependency gating, start and stop order,
restarts and configuration. This process only runs what it is told:

- `rows.load` imports a plugin module and runs its `apply(ctx, config)`;
  `rows.unload` withdraws what the row provided and runs its cleanup;
- `rows.schema` reports what a module declares (config schema, injected
  services, provided services and their method kinds);
- `hosts.provide` / `hosts.withdraw` register rutis services by name;
- the services a row exports are reported with `service(id, handle,
  version)` notifications, and calls to a handle reach the object; one it
  provides without declaring it is announced to a host that asks, with the
  methods its object has (`rows.announce`).

Within the process, `ctx.use(name)` returns the object itself when another
row here provides it, so those calls never leave the process.

A row's `isolate` gives some names a scope label: rows isolating a name with
the same label share it, and the same name under another label (another
instance, say) is another service. Services, host proxies and export slots
are registered by id (`scopes`): the name outside any scope, the name, a
NUL and the label inside one.
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import importlib
import importlib.metadata
import inspect
import os
import sys
import threading
import traceback
from dataclasses import dataclass, field
from typing import Any, Callable

from . import plugin as sdk
from .events import Events, refused_emits
from .peer import Peer, RemoteFuture

# rows.announce: services rows provide without declaring them are announced
# with their methods, to a host whose mount says `announce`.
FEATURES = ["rows.v2", "hosts", "leaf", "scopes", "events", "rows.announce"]
# Test support, not part of the protocol: with RUTIS_TEST_INSPECT set, the
# control method `inspect` reports what this runtime holds (rows, services,
# host proxies, export slots, object handles, event listeners), so a test
# can check that nothing of the rows it unloaded is left in the process.
# Without it the method is unknown.
INSPECT = bool(os.environ.get("RUTIS_TEST_INSPECT"))


def _unhandled_event(error):
    traceback.print_exception(type(error), error, error.__traceback__, file=sys.stderr)
    sys.stderr.flush()
    os._exit(1)


def scoped_id(name: str, label: str | None) -> str:
    """How the service `name` is identified in the scope `label`: the name
    outside any scope (no label: None), the name, a NUL and the label inside
    one. Neither may contain NUL and a label may not be empty, so no two
    pairs share an id."""
    if "\0" in name or (label is not None and "\0" in label):
        raise ValueError(f"service {name!r} or its scope label contains NUL")
    if label == "":
        raise ValueError(f"service {name!r} has an empty scope label")
    return name if label is None else f"{name}\0{label}"


def handle_of(id: str, generation: int) -> str:
    """The handle of a slot's `generation`th object. A scoped id's handles
    are marked with a NUL too: a label may contain `#`."""
    if generation == 1:
        return id
    return f"{id}{chr(0) if chr(0) in id else '#'}{generation}"


class HostProxy:
    """A rutis service: its declared methods call through the session,
    synchronously or as coroutines. `name` is the service's id, which its
    calls target."""

    def __init__(self, peer: Peer, name: str, methods: dict):
        self._peer = peer
        self._name = name
        self._methods = dict(methods)

    def __getattr__(self, method: str) -> Callable:
        if method.startswith("__"):
            raise AttributeError(method)
        kind = self._methods.get(method)
        if kind is None:
            raise AttributeError(f"{self._name}.{method} is not provided by the rutis host")
        target = f"host:{self._name}"
        if kind == "async":

            async def call(*args: Any) -> Any:
                return await self._peer.call_async(target, method, list(args))

        else:

            def call(*args: Any) -> Any:
                result = self._peer.call(target, method, list(args))
                return result

        call.__name__ = method
        return call

    def __repr__(self) -> str:
        return f"<rutis service {self._name}>"


@dataclass
class Slot:
    """An exported service: the sequence of objects it held, by handle."""

    row: str
    methods: set
    name: str = ""
    object: Any = None
    handle: str | None = None
    generation: int = 0
    # Not declared: announced, with the methods of each object.
    announced: bool = False


@dataclass
class Row:
    key: str
    module: str
    config: Any
    exports: dict
    # Service name -> scope label.
    isolate: dict = field(default_factory=dict)
    cleanups: list = field(default_factory=list)
    listeners: list = field(default_factory=list)
    # Ids of the services it provided.
    provided: list = field(default_factory=list)
    # Ids of the slots of services it announced.
    announced: list = field(default_factory=list)
    # Announcements the host has yet to answer, while the row loads: those
    # sent from the session's loop, and from other threads.
    pending: list | None = field(default_factory=list)
    # Guards `pending`, which other threads add to.
    lock: Any = field(default_factory=threading.Lock)

    def pend(self, sent) -> None:
        """Await `sent` before saying the row loaded, while it loads."""
        with self.lock:
            if self.pending is not None:
                self.pending.append(sent)

    def id(self, name: str) -> str:
        """The id of `name` as this row sees it."""
        return scoped_id(name, self.isolate.get(name))


class Context(sdk.Context):
    def __init__(self, runtime: "Runtime", row: Row):
        self._runtime = runtime
        self._row = row

    def use(self, name: str) -> Any:
        return self._runtime.lookup(self._row.id(name))

    def provide(self, name: str, value: Any) -> Callable[[], None]:
        return self._runtime.provide(self._row, name, value)

    def effect(self, cleanup: Callable) -> None:
        self._row.cleanups.append(cleanup)

    def on(self, name: str, listener: Callable) -> Callable[[], None]:
        if self._runtime.rows.get(self._row.key) is not self._row:
            raise RuntimeError(f"row {self._row.key} is not active")
        return self._runtime.events.on(name, listener, self._row.listeners)

    def emit(self, name: str, *args: Any) -> None:
        self._runtime.events.emit(name, *args)


class Runtime:
    def __init__(self) -> None:
        self.peer: Peer | None = None
        self.rows: dict[str, Row] = {}
        self.services: dict[str, tuple[str, Any]] = {}  # id -> (row key, object)
        self.hosts: dict[str, HostProxy] = {}  # id -> proxy
        self.slots: dict[str, Slot] = {}  # id -> slot
        self.handles: dict[str, dict] = {}
        self.version = 0
        self.closing = False
        # The host takes announced services (mount `announce`).
        self.announce = False
        self.events = Events(_unhandled_event)
        self.emits = set()
        # The source file of each plugin module when it was imported.
        self.stamps: dict[str, tuple | None] = {}

    # ── Services ─────────────────────────────────────────────────

    def lookup(self, id: str) -> Any:
        if id in self.services:
            return self.services[id][1]
        if id in self.hosts:
            return self.hosts[id]
        raise LookupError(f"service {id} is not available")

    def provide(self, row: Row, name: str, value: Any) -> Callable[[], None]:
        id = row.id(name)
        if id in self.services:
            raise ValueError(f"service {id} is already provided by row {self.services[id][0]}")
        self.services[id] = (row.key, value)
        row.provided.append(id)
        # Not declared: announced, with the methods it has.
        if self.announce and id not in self.slots and "#" not in name:
            self.slots[id] = Slot(row.key, set(), name, announced=True)
            row.announced.append(id)
        self._refresh(id)

        def withdraw() -> None:
            if self.services.get(id, (None, None))[1] is value:
                del self.services[id]
                if id in row.provided:
                    row.provided.remove(id)
                self._refresh(id)

        return withdraw

    def _refresh(self, id: str) -> None:
        """Report the object now in an exported slot, under a new handle."""
        slot = self.slots.get(id)
        if slot is None:
            return
        provided = self.services.get(id)
        current = provided[1] if provided is not None and provided[0] == slot.row else None
        if current is slot.object:
            return
        if slot.handle is not None:
            self._retire(slot.handle)
        slot.object = current
        slot.handle = None
        if current is not None:
            slot.generation += 1
            slot.handle = handle_of(id, slot.generation)
            self.handles[slot.handle] = {"name": id, "object": current, "current": True, "released": False}
            if slot.announced:
                slot.methods = sdk.methods_of(current)
        self.version += 1
        if self.peer is None or self.closing:
            return
        if not slot.announced:
            self.peer.notify("", "service", [id, slot.handle, self.version])
            return
        # An announced service says whose it is and what it offers, and a
        # row loading answers once rutis has heard it.
        methods = {method: "async" for method in sorted(slot.methods)}
        args = [id, slot.handle, self.version, {"row": slot.row, "methods": methods}]
        try:
            running = asyncio.get_running_loop()
        except RuntimeError:
            running = None
        if running is not self.peer.loop:
            # Provided from another thread: sent from the session's own, and
            # awaited like the others while the row loads.
            if self.peer.closed_error is not None or self.peer.loop.is_closed():
                return
            sent = asyncio.run_coroutine_threadsafe(
                self.peer.call_async("", "service", args), self.peer.loop
            )
            sent.add_done_callback(_ignore)
        else:
            sent = asyncio.ensure_future(self.peer.call_async("", "service", args))
            sent.add_done_callback(_ignore)
        row = self.rows.get(slot.row)
        if row is not None:
            row.pend(sent)

    def _retire(self, handle: str) -> None:
        entry = self.handles.get(handle)
        if entry is None:
            return
        entry["current"] = False
        if entry["released"]:
            del self.handles[handle]

    # ── Rows ─────────────────────────────────────────────────────

    async def load(
        self,
        key: str,
        module: str,
        config: Any,
        exports: dict | None,
        isolate: dict | None = None,
    ) -> None:
        if key in self.rows:
            raise ValueError(f"row {key} is already loaded")
        exports = exports or {}
        row = Row(key, module, config, exports, dict(isolate or {}))
        for name, label in row.isolate.items():
            scoped_id(name, label)
        for name in exports:
            if "#" in name:
                raise ValueError(f"service name {name} cannot be projected")
            if row.id(name) in self.slots:
                raise ValueError(
                    f"service {row.id(name)} is already exported by row {self.slots[row.id(name)].row}"
                )
        plugin = _supported(sdk.load(self.module(module)), module)
        self.rows[key] = row
        for name, methods in exports.items():
            self.slots[row.id(name)] = Slot(key, set(methods), name)
        try:
            cleanup = await _settle(plugin.apply(Context(self, row), config))
            if cleanup is not None:
                if not callable(cleanup):
                    raise TypeError("apply must return a cleanup function or None")
                row.cleanups.append(cleanup)
            for name in exports:
                self._refresh(row.id(name))
            # rutis has published the announced services (or refused one,
            # which fails the row there) before it hears that the row loaded:
            # those its threads provide while it waits too.
            while True:
                with row.lock:
                    pending = row.pending or []
                    row.pending = [] if pending else None
                if not pending:
                    break
                await asyncio.gather(*map(_awaitable, pending), return_exceptions=True)
        except BaseException:
            try:
                await self.unload(key)
            except Exception:
                pass
            raise

    async def unload(self, key: str) -> None:
        row = self.rows.pop(key, None)
        if row is None:
            return
        for off in row.listeners[:]:
            off()
        # Withdrawals first: rutis hears them before the plugin goes away.
        for id in list(row.provided):
            if self.services.get(id, (None,))[0] == key:
                del self.services[id]
                self._refresh(id)
        row.provided.clear()
        for id in [row.id(name) for name in row.exports] + row.announced:
            slot = self.slots.get(id)
            if slot is None or slot.row != key:
                continue
            del self.slots[id]
            if slot.handle is not None:
                self._retire(slot.handle)
        row.announced.clear()
        errors = []
        cleanups = row.cleanups[::-1]
        row.cleanups.clear()
        for cleanup in cleanups:
            try:
                await _settle(cleanup())
            except Exception as error:  # noqa: BLE001 - run every cleanup
                errors.append(error)
        if errors:
            raise errors[0]

    async def update(self, key: str, config: Any) -> None:
        # Leaf plugins have no volatile fields: a new config restarts the row.
        row = self.rows.get(key)
        if row is None:
            raise ValueError(f"row {key} is not loaded")
        await self.unload(key)
        await self.load(key, row.module, config, row.exports, row.isolate)

    def module(self, name: str):
        """The plugin module, imported again when its source file changed
        since (rutis-loader's reload asks for the new code). Only the plugin
        module itself is imported again, not the modules it imports.

        `name` is an entry point of the group `rutis.plugins` (a packaged
        plugin), or else a module name."""
        name = _entry_point(name)[0]
        module = sys.modules.get(name)
        if module is None:
            module = importlib.import_module(name)
        elif name in self.stamps and _stamp(module) != self.stamps[name]:
            importlib.invalidate_caches()
            module = importlib.reload(module)
        self.stamps[name] = _stamp(module)
        return module

    def describe(self, module: str) -> dict:
        plugin = _supported(sdk.load(self.module(module)), module)
        return {
            "config": plugin.config,
            "inject": plugin.inject,
            "provides": plugin.provides,
            "announces": plugin.announces,
            "version": _entry_point(module)[1],
        }

    async def dispose(self) -> None:
        for key in reversed(list(self.rows)):
            try:
                await self.unload(key)
            except Exception:  # noqa: BLE001 - disposal goes on
                pass
        self.events.clear()

    # ── Dispatch ─────────────────────────────────────────────────

    def dispatch(self, target: str, method: str, args: Any) -> Any:
        if self.closing:
            raise RuntimeError("runtime is closing")
        if target == "":
            return self._control(method, args if args is not None else [])
        entry = self.handles.get(target)
        if entry is None:
            raise LookupError(f"unknown or released service object {target}")
        slot = self.slots.get(entry["name"])
        if slot is not None and method not in slot.methods:
            raise AttributeError(f"unknown service method {entry['name']}.{method}")
        if not isinstance(args, list):
            raise TypeError("method arguments must be an array")
        return getattr(entry["object"], method)(*args)

    def _control(self, method: str, args: Any) -> Any:
        if method == "mount":
            names = args.get("emits", [])
            refused = refused_emits(names)
            if refused is not None:
                raise ValueError(refused)
            self.emits = set(names)
            self.announce = args.get("announce") is True
            return {"services": {}, "features": FEATURES}
        if method == "emit":
            name, values = args
            if name not in self.emits:
                raise ValueError(f"event {name} is not declared for emission from rutis")
            return self.events.parallel(name, *(values or []))
        if method == "dispose":
            self.closing = True
            return self._dispose_and_drain()
        if method == "rows.load":
            key, module, config, isolate, _inject, *rest = list(args) + [None] * (6 - len(args))
            labels = {name: label for name, label in (isolate or [])}
            return self.load(key, str(module), config, rest[0] if rest else None, labels)
        if method == "rows.update":
            key, config = args
            return self.update(key, config)
        if method == "rows.unload":
            return self.unload(args[0])
        if method == "rows.schema":
            return self.describe(str(args[0]))
        if method == "hosts.provide":
            # With a label, only rows isolating the name with it see it.
            name, methods, label, *_ = list(args) + [None]
            id = scoped_id(name, label)
            if id in self.hosts:
                raise ValueError(f"host service {id} is already provided")
            self.hosts[id] = HostProxy(self.peer, id, methods or {})
            return None
        if method == "hosts.withdraw":
            self.hosts.pop(args[0], None)
            return None
        if method == "release":
            entry = self.handles.get(args[0])
            if entry is not None:
                entry["released"] = True
                if not entry["current"]:
                    del self.handles[args[0]]
            return None
        if method == "inspect" and INSPECT:
            return {
                "rows": list(self.rows),
                "services": list(self.services),
                "hosts": list(self.hosts),
                "slots": list(self.slots),
                "handles": list(self.handles),
                "listeners": sum(len(listeners) for listeners in self.events.listeners.values()),
            }
        if method == "get":
            handle, prop = args
            entry = self.handles.get(handle)
            if entry is None:
                raise LookupError(f"unknown or released service object {handle}")
            return getattr(entry["object"], prop)
        raise ValueError(f"unknown control method {method}")

    async def _dispose_and_drain(self) -> None:
        await self.dispose()
        await self.peer.drain()


def _ignore(sent) -> None:
    """An announcement nobody awaits: its failure (the session ended) is
    not an error of anything here."""
    if not sent.cancelled():
        sent.exception()


def _awaitable(sent):
    """`sent` as the session's loop awaits it: one another thread sent is a
    `concurrent.futures.Future`."""
    if isinstance(sent, concurrent.futures.Future):
        return asyncio.wrap_future(sent)
    return sent


def _entry_point(name: str) -> tuple[str, str | None]:
    """The module of the plugin `name`, and its package's version: the entry
    point `name` of the group `rutis.plugins`, or else the module `name`."""
    for entry in importlib.metadata.entry_points(group="rutis.plugins", name=name):
        return entry.value.split(":")[0], entry.dist.version if entry.dist else None
    return name, None


def _supported(plugin, name: str):
    if plugin.api > sdk.PLUGIN_API:
        raise RuntimeError(
            f"plugin {name} needs plugin API {plugin.api}; this runtime supports "
            f"{sdk.PLUGIN_API}: upgrade the rutis package where the runtime runs"
        )
    return plugin


def _stamp(module) -> tuple | None:
    path = getattr(module, "__file__", None)
    try:
        stat = os.stat(path)
    except (OSError, TypeError):
        return None
    return (stat.st_mtime_ns, stat.st_size)


async def _settle(value: Any) -> Any:
    while inspect.isawaitable(value) or isinstance(value, RemoteFuture):
        value = await value
    return value

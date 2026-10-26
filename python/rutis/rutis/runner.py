"""The Python runtime: leaf plugins loaded one by one for rutis-loader.

rutis owns the plugin model: dependency gating, start and stop order,
restarts and configuration. This process only runs what it is told:

- `rows.load` imports a plugin module and runs its `apply(ctx, config)`;
  `rows.unload` withdraws what the row provided and runs its cleanup;
- `rows.schema` reports what a module declares (config schema, injected
  services, provided services and their method kinds);
- a plugin definition (`define_plugin`) handed to rutis crosses as a
  `plugin` reference; back home, `plugins.read` describes it and
  `rows.load` takes it as the row's entry instead of a module name
  (`plugin-values`, docs/design-plugin-values-2026-10-25.md §5);
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

Events (#464, docs/design-events-protocol-2026-10-18.md): every `ctx.on`
registers with the host (`events.on` / `events.off`; `rows.load` answers
once the host answered them). The host tells which names and prefixes have
listeners outside this process (`events.elsewhere`); an emission of any
other name is dispatched here (`rutis.events`), the rest through the host
(`events.dispatch`), which calls this process's listeners back with
`events.call`. A row's scope chain is the seventh `rows.load` argument.
Unloading a row withdraws its listeners, then cancels its running ones and
waits for them, at most RUTIS_SHUTDOWN_TIMEOUT. The session requires the
host to declare `events-v2`.
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
from dataclasses import dataclass, field
from typing import Any, Callable

from . import plugin as sdk
from .events import Bus, Emitter, check_scope, discard, plain, reserved, start
from .peer import UNDEFINED, AsyncListener, BindingError, Peer, RemoteFuture

# rows.announce: services rows provide without declaring them are announced
# with their methods, to a host whose mount says `announce`. Events are not
# a feature any more: from rutis 0.13 they are the `events-v2` capability.
# plugin-values: a plugin definition this runtime handed over comes back to
# be described (`plugins.read`) and loaded as a row's entry
# (docs/design-plugin-values-2026-10-25.md §5).
FEATURES = ["rows.v2", "hosts", "leaf", "scopes", "rows.announce", "plugin-values"]
# Test support, not part of the protocol: with RUTIS_TEST_INSPECT set, the
# control method `inspect` reports what this runtime holds (rows, services,
# host proxies, export slots, object handles, event listeners), so a test
# can check that nothing of the rows it unloaded is left in the process.
# Without it the method is unknown.
INSPECT = bool(os.environ.get("RUTIS_TEST_INSPECT"))


DEADLINE = "RUTIS_SHUTDOWN_TIMEOUT"


def shutdown_deadline() -> float:
    """How long cleanups, and a row's running listeners on unload, may
    take, in seconds: RUTIS_SHUTDOWN_TIMEOUT (rutis-host passes its own
    deadline on), 10 by default."""
    try:
        seconds = float(os.environ.get(DEADLINE, ""))
    except ValueError:
        return 10.0
    return seconds if 0 < seconds < float("inf") else 10.0


def removed(field: str) -> str:
    """What a mount still carrying the 0.12 event declarations is told."""
    return (
        f"the mount argument {field} was removed in rutis 0.13: listeners register themselves; "
        "emit with ctx.emit / parallel / serial / waterfall / bail (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)"
    )


def old_host(name: str) -> str:
    """What a plugin providing the undeclared service `name` is told when
    the host, older than 0.12, mounted without `announce`: such a host
    does not take the service, which stays in this process."""
    return (
        f"rutis: service {name} is not declared in provides, and this host (older than 0.12) "
        "does not take undeclared services; declare it or upgrade rutis-host"
    )


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
    # A module's name, or a plugin definition this runtime handed over.
    module: Any
    config: Any
    exports: dict
    # Service name -> scope label.
    isolate: dict = field(default_factory=dict)
    # Its scope chain, `[{id, group}, ...]` (`[]`: the root): its listeners
    # hear what is emitted there and inside, its emissions come from there.
    scope: list = field(default_factory=list)
    # The plugin's declared name (`define_plugin(name=)`); None: the row's id.
    plugin: str | None = None
    cleanups: list = field(default_factory=list)
    # Ids of the services it provided.
    provided: list = field(default_factory=list)
    # Ids of the slots of services it announced.
    announced: list = field(default_factory=list)
    # Announcements and registrations the host has yet to answer, while
    # the row loads: those sent from the session's loop, and from other
    # threads; (sent, whether it is a registration).
    pending: list | None = field(default_factory=list)
    # Guards `pending`, which other threads add to.
    lock: Any = field(default_factory=threading.Lock)

    def pend(self, sent, registration: bool = False) -> bool:
        """Await `sent` before saying the row loaded, while it loads (a
        refused registration fails the load); False once it loaded."""
        with self.lock:
            if self.pending is None:
                return False
            self.pending.append((sent, registration))
            return True

    def id(self, name: str) -> str:
        """The id of `name` as this row sees it."""
        return scoped_id(name, self.isolate.get(name))


class Context(sdk.Context):
    def __init__(self, runtime: "Runtime", row: Row):
        self._runtime = runtime
        self._row = row
        self._emitter = Emitter(lambda *request: runtime.send(row, *request))

    def use(self, name: str) -> Any:
        return self._runtime.lookup(self._row.id(name))

    def provide(self, name: str, value: Any) -> Callable[[], None]:
        return self._runtime.provide(self._row, name, value)

    def effect(self, cleanup: Callable) -> None:
        self._row.cleanups.append(cleanup)

    def on(self, name: str, listener: Callable, *, prepend: bool = False, global_: bool = False) -> Callable[[], None]:
        if self._runtime.rows.get(self._row.key) is not self._row:
            raise RuntimeError(f"row {self._row.key} is not active")
        return self._runtime.listen(self._row, name, listener, prepend, global_)

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
        # Mounted by a host; one without `announce` is older than 0.12.
        self.mounted = False
        # The undeclared names a plugin was told such a host does not take.
        self.told: set = set()
        # This process's listeners; a failure nobody receives (an `emit`
        # listener's) is a line on stderr, never the end of the process.
        self.bus = Bus()
        # The session's loop, where listeners run and emissions dispatch;
        # other threads' are marshalled onto it.
        self.loop: asyncio.AbstractEventLoop | None = None
        # Names and prefixes with listeners outside this process (the host
        # keeps them up to date, `events.elsewhere`): emitting them goes
        # through the host.
        self.elsewhere: set = set()
        self.prefixes: set = set()
        # Emits of each name sent through the host and not answered yet:
        # later ones follow them there, so they start in order.
        self.unanswered: dict[str, int] = {}
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
        # Not declared: announced, with the methods it has; a host older
        # than 0.12 does not take it, and the plugin is told, once per name.
        if id not in self.slots and "#" not in name:
            if self.announce:
                self.slots[id] = Slot(row.key, set(), name, announced=True)
                row.announced.append(id)
            elif self.mounted and name not in self.told:
                self.told.add(name)
                print(old_host(name), file=sys.stderr, flush=True)
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
        module: Any,
        config: Any,
        exports: dict | None,
        isolate: dict | None = None,
        scope: list | None = None,
        id: str | None = None,
    ) -> None:
        if key in self.rows:
            raise ValueError(f"row {key} is already loaded")
        if self.loop is None:
            self.loop = asyncio.get_running_loop()
        # Descriptors and reported lines call the row by its id (§4.2).
        self.bus.row_ids[key] = id if isinstance(id, str) else key
        exports = exports or {}
        row = Row(key, module, config, exports, dict(isolate or {}), check_scope(scope))
        for name, label in row.isolate.items():
            scoped_id(name, label)
        for name in exports:
            if "#" in name:
                raise ValueError(f"service name {name} cannot be projected")
            if row.id(name) in self.slots:
                raise ValueError(
                    f"service {row.id(name)} is already exported by row {self.slots[row.id(name)].row}"
                )
        plugin = self._plugin(module)
        row.plugin = plugin.name
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
            # which fails the row there) and registered its listeners before
            # it hears that the row loaded: those its threads provide while
            # it waits too. A refused registration fails the load (§4.4).
            while True:
                with row.lock:
                    pending = row.pending or []
                    row.pending = [] if pending else None
                if not pending:
                    break
                outcomes = await asyncio.gather(*(_awaitable(sent) for sent, _ in pending), return_exceptions=True)
                for (_sent, registration), outcome in zip(pending, outcomes):
                    if registration and isinstance(outcome, Exception):
                        raise outcome
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
        # Its listeners start nothing any more; an `events.call` for one of
        # them is refused (the host withdrew them already: telling it again
        # does no harm).
        self._withdrawn(self.bus.withdraw_row(key))
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
        # Its running listeners are cancelled and waited for, at most the
        # shutdown deadline (§7.1 step 4), before its cleanups run.
        await self.bus.stop(key, shutdown_deadline())
        self.bus.row_ids.pop(key, None)
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
        id = self.bus.row_id(key)
        await self.unload(key)
        await self.load(key, row.module, config, row.exports, row.isolate, row.scope, id)

    def module(self, name: str, fresh: bool = True):
        """The plugin module, imported again when its source file changed
        since (rutis-loader's reload describes the row again, asking for the
        new code). Only the plugin module itself is imported again, not the
        modules it imports.

        With `fresh` false (a row loading), the module as this process last
        imported it, unchanged: a row that loads again for another reason (a
        service it uses came back, its config changed) runs the code it was
        described with, not an edit made since, which may not even parse;
        the edit runs once a reload describes it (#665).

        `name` is an entry point of the group `rutis.plugins` (a packaged
        plugin), or else a module name."""
        name = _entry_point(name)[0]
        module = sys.modules.get(name)
        if module is None:
            module = importlib.import_module(name)
        elif not fresh and name in self.stamps:
            return module
        elif name in self.stamps and _stamp(module) != self.stamps[name]:
            importlib.invalidate_caches()
            module = importlib.reload(module)
        self.stamps[name] = _stamp(module)
        return module

    def _plugin(self, entry: Any) -> sdk.Plugin:
        """The plugin of a row: its module's, or a plugin definition this
        runtime handed over (sent back home as itself)."""
        if isinstance(entry, str):
            return _supported(sdk.load(self.module(entry, fresh=False)), entry)
        plugin = _definition(entry)
        return _supported(plugin, plugin.name or "plugin definition")

    def describe_definition(self, value: Any) -> dict:
        """What a plugin definition declares (`plugins.read`), as `describe`
        says of a module, with its name."""
        plugin = self._plugin(_definition(value))
        return {
            "name": plugin.name,
            "config": plugin.config,
            "inject": plugin.inject,
            "provides": plugin.provides,
            "announces": plugin.announces,
            "version": None,
        }

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
        self.bus.clear()

    # ── Events ───────────────────────────────────────────────────

    def _live(self) -> bool:
        """A session with the host is on: listeners register with it, and
        emissions of names listened to elsewhere go through it. Without
        one, elsewhere is empty and everything stays here."""
        return self.peer is not None and self.peer.closed_error is None

    def _here(self) -> bool:
        """Whether this runs on the session's loop (not another thread)."""
        try:
            return asyncio.get_running_loop() is self.loop
        except RuntimeError:
            return self.loop is None

    def listen(self, row: Row, name: str, listener: Callable, prepend: bool, global_: bool) -> Callable[[], None]:
        """Add a listener of the row here and register it with the host,
        which answers before the row is said to have loaded (§4.4); one it
        refuses then fails the load. From any thread: the frame goes out
        from the loop, in call order with that thread's emissions."""
        reserved(name, listening=True)
        plugin = row.plugin or self.bus.row_id(row.key)
        entry = self.bus.add(name, row.key, plugin, listener, row.scope, prepend, global_)

        def off() -> None:
            if self.bus.withdraw(entry):
                self._withdrawn([entry.id])

        if self._live():
            registration = {
                "id": entry.id,
                "name": name,
                "row": row.key,
                "plugin": plugin,
                "kind": entry.kind,
                "prepend": entry.prepend,
                "global": entry.global_,
            }
            try:
                sent = self._request("events.on", [registration])
            except Exception as error:  # noqa: BLE001 - the session ended meanwhile
                if self._live():
                    self.bus.report("registration", name, row.key, error)
            else:
                # Awaited by the row's load, which it fails; reported only
                # when the row was running already.
                if not row.pend(sent, registration=True):
                    sent.add_done_callback(lambda done: self._registered(done, name, row.key))
        return off

    def _request(self, method: str, args: list):
        """`peer.request`, from any thread: from another, sent from the
        loop in call order with what that thread marshals there next (a
        `concurrent.futures.Future` of the reply)."""
        if self._here():
            return self.peer.request("", method, args)
        reply = concurrent.futures.Future()

        def send() -> None:
            try:
                sent = self.peer.request("", method, args)
            except BaseException as error:  # noqa: BLE001 - the reply's
                reply.set_exception(error)
            else:
                _chain(sent, reply)

        self.loop.call_soon_threadsafe(send)
        return reply

    def _registered(self, done, name: str, row: str) -> None:
        if done.cancelled() or done.exception() is None or not self._live():
            return
        self.bus.report("registration", name, row, done.exception())

    def _withdrawn(self, ids: list) -> None:
        if ids and self._live():
            self.peer.notify("", "events.off", [ids])

    def _via_host(self, name: str, mode: str) -> bool:
        """Whether an emission goes through the host (§5.2): a listener of
        the name may be elsewhere, or (`emit`) an earlier emit of it there
        is not answered yet."""
        if not self._live():
            return False
        if name in self.elsewhere or any(name.startswith(prefix) for prefix in self.prefixes):
            return True
        return mode == "emit" and self.unanswered.get(name, 0) > 0

    def send(self, row: Row, mode: str, name: str, args: list, terminal: Callable | None, keep: Callable | None) -> Any:
        """An emission of the row: here (the fast path), or through the host,
        which dispatches over every listener and calls this process's own
        back. Same results either way. A row that unloaded emits nothing
        (BindingError, refused at once like a reserved name)."""
        if self.rows.get(row.key) is not row:
            raise BindingError(f"row {row.key} is not loaded in this runtime")
        if not self._here():
            return self._send_from_thread(row, mode, name, args, terminal, keep)
        if not self._via_host(name, mode):
            return self.bus.dispatch(mode, name, args, keep, terminal, row.key)
        request = {"row": row.key, "name": name, "mode": mode, "args": args}
        if keep is not None:
            request["filter"] = _selector(keep)
        if terminal is not None:
            request["terminal"] = terminal
        if mode == "emit":
            self._emit_via_host(row.key, name, request)
            return None
        if mode == "bail":
            return plain(self.peer.call("", "events.dispatch", [request]))
        # Sent now, as the fast path takes its snapshot now.
        return start(self._dispatch_via_host(request))

    def _send_from_thread(self, row: Row, mode: str, name: str, args: list, terminal, keep) -> Any:
        """An emission from another thread, made on the loop in call order:
        `emit` returns at once, `bail` blocks this thread for the result,
        the others return an awaitable for this thread's running loop (a
        `concurrent.futures.Future` without one)."""
        result = concurrent.futures.Future()

        def here() -> None:
            try:
                sent = self.send(row, mode, name, args, terminal, keep)
            except BaseException as error:  # noqa: BLE001 - the emitter's
                if mode == "emit":
                    self.bus.report("emit", name, row.key, error)
                else:
                    result.set_exception(error)
                return
            if mode in ("emit", "bail"):
                result.set_result(sent)
            else:
                _chain(start(sent), result)

        self.loop.call_soon_threadsafe(here)
        if mode == "emit":
            return None
        if mode == "bail":
            return result.result()
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            return result
        return asyncio.wrap_future(result)

    async def _dispatch_via_host(self, request: dict) -> Any:
        return plain(await _settle(await self.peer.call_async("", "events.dispatch", [request])))

    def _emit_via_host(self, row: str, name: str, request: dict) -> None:
        self.unanswered[name] = self.unanswered.get(name, 0) + 1

        def failed(error: BaseException) -> None:
            self._answered(name)
            self.bus.report("emit", name, row, error)

        def settled(done) -> None:
            if not done.cancelled() and done.exception() is not None:
                failed(done.exception())
            else:
                self._answered(name)

        def answered(ok: bool, value: Any) -> None:
            # Taken as the reply's frame is read, before the next one: an
            # emit a later frame starts sees it answered (§5.5).
            if not ok:
                failed(value)
            elif isinstance(value, RemoteFuture):
                # An older host answers with a future: its settlement.
                async def awaited() -> Any:
                    return await value

                self.peer.loop.create_task(awaited()).add_done_callback(settled)
            else:
                self._answered(name)

        try:
            self.peer.request_answered("", "events.dispatch", [request], answered)
        except Exception as error:  # noqa: BLE001 - contained and reported
            failed(error)

    def _answered(self, name: str) -> None:
        left = self.unanswered.get(name, 1) - 1
        if left > 0:
            self.unanswered[name] = left
        else:
            self.unanswered.pop(name, None)

    def _elsewhere(self, change: Any) -> None:
        """The names and prefixes listened to outside this process changed."""
        if not isinstance(change, dict):
            raise ValueError("invalid events.elsewhere")
        lists = {}
        for part in ("add", "remove", "addPrefixes", "removePrefixes"):
            items = change.get(part, [])
            if items is None or items is UNDEFINED:
                items = []
            if not isinstance(items, list) or not all(isinstance(item, str) for item in items):
                raise ValueError(f"invalid events.elsewhere {part}")
            lists[part] = items
        self.elsewhere.difference_update(lists["remove"])
        self.elsewhere.update(lists["add"])
        self.prefixes.difference_update(lists["removePrefixes"])
        self.prefixes.update(lists["addPrefixes"])

    def _call_listener(self, request: Any) -> Any:
        """`events.call`: run one listener of this process for a dispatch
        the host runs. Its return value goes back as is; in `emit` mode,
        undefined once it finished, its failure reported here."""
        if not isinstance(request, dict):
            raise ValueError("invalid events.call")
        id, name, mode = request.get("listener"), request.get("name"), request.get("mode")
        entry = self.bus.listener(id) if isinstance(id, str) else None
        if entry is None:
            raise BindingError(f"listener {id} of {name} is withdrawn")
        name = entry.name
        args = request.get("args")
        args = [plain(arg) for arg in args] if isinstance(args, list) else []
        if mode in ("bail", "waterfall-sync"):
            if mode == "waterfall-sync":
                following = request.get("next")
                args.append(lambda: plain(following()))
            refused = AsyncListener(
                f"listener of {name} in row {self.bus.row_id(entry.row)} is asynchronous and cannot run in a synchronous {mode}"
            )
            if entry.kind == "async":
                raise refused
            value = entry.listener(*args)
            if inspect.isawaitable(value):
                discard(value)
                raise refused
            return value
        if mode == "waterfall":
            following = request.get("next")
            # Started at once, as on the fast path.
            args.append(lambda: start(_next(following)))
        elif mode not in ("emit", "parallel", "serial"):
            raise ValueError(f"unknown event mode {mode}")
        return self._invocation(entry, name, mode, args)

    async def _invocation(self, entry, name: str, mode: str, args: list) -> Any:
        # Started at once (the peer starts it eagerly): an `emit` listener
        # runs to its first await before the next frame is read (§5.5).
        task = asyncio.current_task()
        if task is not None:
            self.bus.track(entry.row, task)
        try:
            value = await _settle(entry.listener(*args))
        except asyncio.CancelledError:
            # Cancelled by its row's unload (§7.1): in `emit` mode not a
            # failure, and the reply is undefined as always (§5.3).
            if mode != "emit":
                raise
            return UNDEFINED
        except Exception as error:
            if mode != "emit":
                raise
            self.bus.report("listener", name, entry.row, error)
            return UNDEFINED
        return UNDEFINED if mode == "emit" else value

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
        if method == "events.call":
            return self._call_listener(args[0] if args else None)
        if method == "events.elsewhere":
            self._elsewhere(args[0] if args else None)
            return None
        if method == "mount":
            for removed_field in ("emits", "events"):
                if removed_field in args:
                    raise ValueError(removed(removed_field))
            # The runtime's name, which filters see in this process's
            # listeners' descriptors.
            runtime = args.get("runtime")
            self.bus.runtime = runtime if isinstance(runtime, str) else None
            self.announce = args.get("announce") is True
            self.mounted = True
            return {"services": {}, "features": FEATURES}
        if method == "dispose":
            self.closing = True
            return self._dispose_and_drain()
        if method == "rows.load":
            # From rutis 0.13 the seventh argument is `{ scope, id }`.
            key, module, config, isolate, _inject, exports, row = (list(args) + [None] * 7)[:7]
            labels = {name: label for name, label in (isolate or [])}
            scope = row.get("scope") if isinstance(row, dict) else None
            id = row.get("id") if isinstance(row, dict) else None
            # A module's name, or a plugin definition this runtime handed over.
            entry = module if isinstance(module, sdk.Plugin) else str(module)
            return self.load(key, entry, config, exports, labels, scope, id)
        if method == "rows.update":
            key, config = args
            return self.update(key, config)
        if method == "rows.unload":
            return self.unload(args[0])
        if method == "rows.schema":
            return self.describe(str(args[0]))
        if method == "plugins.read":
            return self.describe_definition(args[0] if args else None)
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
                "listeners": len(self.bus.entries),
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


def _definition(value: Any) -> sdk.Plugin:
    if not isinstance(value, sdk.Plugin):
        raise TypeError("not a plugin definition: expected a define_plugin result")
    return value


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


def _selector(keep: Callable) -> Callable:
    """What the host calls once per dispatch with the descriptors of the
    candidates (non-`global`, in order): the plugin's filter asked of each,
    here, in order; a boolean each (§6)."""

    def select(descriptors: list) -> list:
        # The host marks the emitter's own session's entries with their
        # `listener` id; the plugin sees descriptors as on the fast path.
        return [bool(keep(_descriptor(descriptor))) for descriptor in descriptors]

    return select


def _descriptor(descriptor: Any) -> Any:
    if isinstance(descriptor, dict) and "listener" in descriptor:
        return {key: value for key, value in descriptor.items() if key != "listener"}
    return descriptor


def _chain(source: asyncio.Future, target: concurrent.futures.Future) -> None:
    """Settle `target` (another thread's) as `source` settles."""

    def settle(done: asyncio.Future) -> None:
        if target.done():
            return
        if done.cancelled():
            target.cancel()
        elif done.exception() is not None:
            target.set_exception(done.exception())
        else:
            target.set_result(done.result())

    source.add_done_callback(settle)


async def _next(following: Any) -> Any:
    """A waterfall listener's `next` when the host runs the waterfall: the
    rest of the chain, which the host's function reference runs."""
    return plain(await _settle(await following.call_async()))

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
import importlib.util
import inspect
import os
import sys
import threading
import traceback
from dataclasses import dataclass, field
from typing import Any, Callable

from . import plugin as sdk
from ._output import BlockingWatch, install_prefix, say
from .config import configured
from ._unhandled import (
    as_row,
    current_row,
    failure_text,
    install_exit,
    kind_text,
    record_task_rows,
    report_text,
    row_of,
    unhandled_error,
)
from .events import Bus, Emitter, check_scope, discard, plain, reserved, start
from .peer import UNDEFINED, AsyncListener, BindingError, LoadError, Peer, RemoteFuture, encode_error

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


def undeclared_use(name: str) -> str:
    """What a plugin using `name`, which neither its `inject` nor its row's
    declares, is told when the service is not there for it: what to declare,
    not that it is missing (#676). The same in every runtime and testing
    tool."""
    return (
        f"service {name} is not available to this plugin, which does not declare it "
        f"in inject: add '{name}' to its inject"
    )


def not_declared(cls: type, name: str) -> BaseException:
    """The error of a plugin using `name` without declaring it (#676), of
    class `cls`, with the code `not-declared` and `{service}`."""
    error = cls(undeclared_use(name))
    error.code = "not-declared"
    error.data = {"service": name}
    return error


def unavailable(name: str, reason: str | None = None) -> LookupError:
    """No provider of the service `name` is there now: the code
    `unavailable` and `{service, reason?}`."""
    error = LookupError(f"service {name} is not available" + (f" ({reason})" if reason else ""))
    error.code = "unavailable"
    error.data = {"service": name, **({"reason": reason} if reason else {})}
    return error


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
        service = self._name.split("\0", 1)[0]
        kind = self._methods.get(method)
        if kind is None:
            raise no_method(AttributeError, service, method, self._methods)
        target = f"host:{self._name}"
        peer = self._peer
        if kind == "async":

            async def call(*args: Any) -> Any:
                return plain(await peer.call_async(target, method, list(args)))

        else:

            def call(*args: Any) -> Any:
                return plain(peer.call(target, method, list(args)))

        # A traceback through it, and its repr, show the service and the
        # method called, not this SDK's code.
        qualname = f"{service}.{method}"
        code = call.__code__.replace(co_name=qualname)
        if hasattr(code, "co_qualname"):
            code = code.replace(co_qualname=qualname)
        call.__code__ = code
        call.__name__ = method
        call.__qualname__ = qualname
        return call

    def __repr__(self) -> str:
        return f"<service {self._name.split(chr(0), 1)[0]}>"


def no_method(cls: type, service: str, method: str, methods) -> BaseException:
    """`service` has no method `method`: the error a call of it raises, of
    class `cls`, with the code `no-method` and `{service, method,
    methods}` (the diagnostics design §4.3), the same text in every
    language."""
    names = sorted(methods)
    listed = f"methods: {', '.join(names)}" if names else "no methods"
    error = cls(f"{service} has no method {method} ({listed})")
    error.code = "no-method"
    error.data = {"service": service, "method": method, "methods": names}
    return error


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
    # What messages call the row: its id, or the key when it was not named.
    named: str = ""
    cleanups: list = field(default_factory=list)
    # Ids of the services it provided.
    provided: list = field(default_factory=list)
    # Ids of the slots of services it announced.
    announced: list = field(default_factory=list)
    # The names it declares it uses: its plugin's `inject`, and its row's.
    inject: set = field(default_factory=set)
    # The row's own `inject` (extra names it waits for).
    row_inject: list = field(default_factory=list)
    # Announcements and registrations the host has yet to answer, while
    # the row loads: those sent from the session's loop, and from other
    # threads; (sent, whether it is a registration).
    pending: list | None = field(default_factory=list)
    # While it loads: set with why when its code leaves an exception
    # unhandled, which fails the load (#688).
    failing: Any = None
    # Failed by an exception its code left unhandled, once loaded: rutis
    # was told (`rows.failed`, #688).
    failed: bool = False
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
        try:
            return self._runtime.lookup(self._row.id(name))
        except LookupError:
            # Not given to it, though it may run: say so (#676).
            if name not in self._row.inject:
                raise not_declared(LookupError, name) from None
            raise unavailable(name) from None

    def provide(self, name: str, value: Any) -> Callable[[], None]:
        return self._runtime.provide(self._row, name, value)

    def effect(self, cleanup: Callable) -> None:
        self._row.cleanups.append(cleanup)

    def on(self, name: str, listener: Callable, *, prepend: bool = False, global_: bool = False) -> Callable[[], None]:
        if self._runtime.rows.get(self._row.key) is not self._row:
            raise RuntimeError(f"row {self._row.named or self._row.key} is not active")
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
        # Listeners run as their row's code (#688).
        self.bus.within = as_row
        # A row's synchronous method holding the loop for 5 s is said
        # (#678).
        self.blocking = BlockingWatch(self._runtime_name)
        # The loop whose unhandled exceptions this runtime takes (#688).
        self._handling: asyncio.AbstractEventLoop | None = None
        try:
            self._take_unhandled(asyncio.get_running_loop())
        except RuntimeError:
            pass  # no loop yet: the first row load takes it
        _exit_named(self)

    # ── Services ─────────────────────────────────────────────────

    def lookup(self, id: str) -> Any:
        if id in self.services:
            return self.services[id][1]
        if id in self.hosts:
            return self.hosts[id]
        raise unavailable(id.split("\0", 1)[0])

    def provide(self, row: Row, name: str, value: Any) -> Callable[[], None]:
        id = row.id(name)
        if id in self.services:
            raise ValueError(f"{name} is already provided by {self.bus.row_id(self.services[id][0])}")
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
                say(old_host(name))
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
        inject: list | None = None,
    ) -> None:
        if key in self.rows:
            raise ValueError(f"row {key} is already loaded")
        if self.loop is None:
            self.loop = asyncio.get_running_loop()
        self._take_unhandled(asyncio.get_running_loop())
        # Descriptors and reported lines call the row by its id (§4.2).
        self.bus.row_ids[key] = id if isinstance(id, str) else key
        exports = exports or {}
        row = Row(key, module, config, exports, dict(isolate or {}), check_scope(scope))
        row.named = self.bus.row_ids[key]
        for name, label in row.isolate.items():
            scoped_id(name, label)
        for name in exports:
            if "#" in name:
                raise ValueError(f"service name {name} cannot be projected")
            if row.id(name) in self.slots:
                raise ValueError(f"{name} is already provided by {self.bus.row_id(self.slots[row.id(name)].row)}")
        plugin = self._plugin(module)
        row.plugin = plugin.name
        row.row_inject = [name for name in inject or [] if isinstance(name, str)]
        row.inject = set(plugin.inject) | set(row.row_inject)
        self.rows[key] = row
        for name, methods in exports.items():
            self.slots[row.id(name)] = Slot(key, set(methods), name)
        # The row's code runs as the row's: what its tasks leave unhandled
        # names it (#688). While it loads, that fails the load.
        # The loop running this load (a test may load on another loop than
        # the one this runtime first saw).
        row.failing = asyncio.get_running_loop().create_future()
        loading = start(as_row(key, lambda: self._apply(row, plugin, config, exports)))
        try:
            await asyncio.wait({loading, row.failing}, return_when=asyncio.FIRST_COMPLETED)
            if not loading.done():
                loading.cancel()
                raise row.failing.exception()
            loading.result()
            # Failed once loaded, before this answers: the load fails.
            if row.failing.done() and not row.failing.cancelled():
                raise row.failing.exception()
        except BaseException:
            if not loading.done():
                loading.cancel()
            try:
                await self.unload(key)
            except Exception:
                pass
            raise
        finally:
            failing, row.failing = row.failing, None
            if not failing.done():
                failing.cancel()
            elif not failing.cancelled():
                failing.exception()  # taken: nobody else reads it

    async def _apply(self, row: Row, plugin: sdk.Plugin, config: Any, exports: dict) -> None:
        """Apply the row's plugin; done once rutis heard what it provided
        and registered."""
        # Defaults filled and the config validated against the plugin's
        # schema before `apply` (config schema design §2.2): a config that
        # does not match fails the row with a `bad-config` BindingError.
        # Extra keys are the host's loader's to warn about.
        config = configured(plugin.config, config)
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

    # ── Errors nobody handled (#688) ─────────────────────────────

    def _take_unhandled(self, loop: asyncio.AbstractEventLoop) -> None:
        """Take `loop`'s exceptions nobody handled (once per loop)."""
        if self._handling is loop:
            return
        self._handling = loop
        loop.set_exception_handler(self._unhandled)
        record_task_rows(loop)

    def _unhandled(self, loop: asyncio.AbstractEventLoop, context: dict) -> None:
        error = context.get("exception")
        if not isinstance(error, Exception):
            loop.default_exception_handler(context)
            return
        self.charge("unhandled exception", error, row_of(context))

    def traced(self, peer: Peer) -> None:
        """Functions and objects rows hand over through `peer` run as the
        row's code when called (#688)."""
        peer.owner = current_row.get
        peer.within = as_row

    def charged_row(self, key: str | None, error: BaseException) -> str | None:
        """The row an error traced to row `key` is charged to: that row,
        unless the innermost plugin file in its traceback is another row's
        only. Then it came from code of a row this one called directly, in
        this process, and which of the two left it unhandled cannot be told
        (the traceback says where it was raised, not who dropped it): no row
        is charged."""
        if key is None:
            return None
        files = {row_key: _module_file(row.module) for row_key, row in list(self.rows.items())}
        for frame in reversed(traceback.extract_tb(error.__traceback__)):
            owners = [row_key for row_key, file in files.items() if file is not None and file == frame.filename]
            if not owners:
                continue
            return key if key in owners else None
        return key

    def _runtime_name(self) -> str:
        return self.bus.runtime or "py"

    def output_name(self) -> str:
        """Who writes now: the id of the row whose code runs, else this
        runtime's name (#678)."""
        key = current_row.get()
        if key is None:
            return self._runtime_name()
        return self.bus.row_id(key) or key

    def prefix_output(self) -> None:
        """Start each line written to stdout and stderr with the name of
        whoever writes it ([`output_name`])."""
        install_prefix(self.output_name)

    def charge(self, kind: str, error: BaseException, key: str | None) -> None:
        """An exception nobody handled, traced to row `key` (None: to no
        row): the row fails (rutis restarts it, and the restart fails with
        why); one traced to no row is reported here. The process goes on."""
        key = self.charged_row(key, error)
        row = self.rows.get(key) if key is not None else None
        name = self._runtime_name()
        if row is None or self.closing or self.peer is None:
            say(f"runtime {name}: an unhandled error from an unknown row: {report_text(error)}")
            return
        why = failure_text(kind, error)
        if row.failing is not None:
            if not row.failing.done():
                row.failing.set_exception(unhandled_error(kind, error))
            return
        # Once: the row is on its way out; what else it leaves unhandled
        # until it has gone is written here.
        if row.failed:
            say(f"runtime {name}: row {self.bus.row_id(key)}: {why}")
            return
        row.failed = True

        def told(done) -> None:
            # A host older than 0.18 fails no row so: the row goes on.
            if done.cancelled() or done.exception() is not None:
                say(f"runtime {name}: row {self.bus.row_id(key)}: {why}")

        try:
            # What it was, and the error as a raised one crosses: its code,
            # data and traceback, which the host prints under it.
            try:
                encoded = encode_error(error)
            except Exception:  # noqa: BLE001 - the text alone then
                encoded = None
            args = [key, kind_text(kind, error)] + ([encoded] if encoded is not None else [])
            self._request("rows.failed", args).add_done_callback(told)
        except Exception:  # noqa: BLE001 - the session is gone
            say(f"runtime {name}: row {self.bus.row_id(key)}: {why}")

    def name_row(self, key: str | None, files: list[str]) -> str | None:
        """The id of the row whose code runs (`key`), else of the one row
        whose plugin module is among `files` (a thread a plugin started has
        a context of its own); None when it cannot tell."""
        if key is not None and key in self.rows:
            return self.bus.row_id(key)
        found = set()
        for row_key, row in list(self.rows.items()):
            file = _module_file(row.module)
            if file is not None and file in files:
                found.add(row_key)
        if len(found) == 1:
            return self.bus.row_id(found.pop())
        return None

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
        await self.load(key, row.module, config, row.exports, row.isolate, row.scope, id, row.row_inject)

    def module(self, name: str, fresh: bool = True, check: Callable[[Any], Any] | None = None):
        """The plugin module, imported again when its source file changed
        since (rutis-loader's reload describes the row again, asking for the
        new code). Only the plugin module itself is imported again, not the
        modules it imports.

        A module imported again is reloaded in place: when that raises (an
        error while importing it), or `check` raises of it (it declares no
        plugin), it is put back as it was, not left half updated.

        With `fresh` false (a row loading under `rutis-host dev`), the module
        as this process last imported it, unchanged: a row that loads again
        for another reason (a service it uses came back, its config changed)
        runs the code it was described with, not an edit made since, which
        may not even import; the edit runs once a reload describes it
        (#665).

        `name` is an entry point of the group `rutis.plugins` (a packaged
        plugin), or else a module name."""
        name = _entry_point(name)[0]
        module = sys.modules.get(name)
        if module is None:
            try:
                module = importlib.import_module(name)
            except ModuleNotFoundError as error:
                # No such module at all: said as it is (the plugin has no file).
                if error.name == name:
                    raise
                raise cannot_load(error, name) from error
            except Exception as error:
                raise cannot_load(error, name) from error
        elif not fresh and name in self.stamps:
            return module
        elif name in self.stamps and _stamp(module) != self.stamps[name]:
            saved = dict(module.__dict__)
            try:
                importlib.invalidate_caches()
                try:
                    module = importlib.reload(module)
                except Exception as error:
                    raise cannot_load(error, name) from error
                if check is not None:
                    check(module)
            except BaseException:
                module.__dict__.clear()
                module.__dict__.update(saved)
                raise
        self.stamps[name] = _stamp(module)
        return module

    def _plugin(self, entry: Any) -> sdk.Plugin:
        """The plugin of a row: its module's, or a plugin definition this
        runtime handed over (sent back home as itself). Under `rutis-host
        dev` (RUTIS_DEV=1), a module's as last described; else as its file
        is now."""
        if isinstance(entry, str):
            check = lambda module: _supported(sdk.load(module), entry)  # noqa: E731
            fresh = os.environ.get("RUTIS_DEV") != "1"
            return check(self.module(entry, fresh=fresh, check=check))
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
        check = lambda loaded: _supported(sdk.load(loaded), module)  # noqa: E731
        plugin = check(self.module(module, check=check))
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
            raise BindingError(f"row {row.named or row.key} is not loaded in this runtime")
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
            raise BindingError(f"the listener of {name} is withdrawn")
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
            value = as_row(entry.row, lambda: entry.listener(*args), awaited=False)
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
        # Its task's context is its own: the listener runs as its row's.
        current_row.set(entry.row)
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
            # A control call is no row's, also arriving while a row's
            # synchronous call waits (#688).
            return as_row(None, lambda: self._control(method, args if args is not None else []))
        entry = self.handles.get(target)
        if entry is None:
            # A handle is the service's id, maybe with `#<generation>`.
            raise _binding(unavailable(target.split("\0", 1)[0].split("#", 1)[0], "withdrawn"))
        service = entry["name"].split("\0", 1)[0]
        slot = self.slots.get(entry["name"])
        methods = slot.methods if slot is not None else sdk.methods_of(entry["object"])
        if method not in methods:
            raise no_method(BindingError, service, method, methods)
        function = getattr(entry["object"], method)
        if not isinstance(args, list):
            raise TypeError("method arguments must be an array")
        check_arguments(service, method, function, args)
        # The method runs as the code of the row whose service it is (#688).
        owner = slot.row if slot is not None else None
        if owner is None and entry["name"] in self.services:
            owner = self.services[entry["name"]][0]
        shown = self._runtime_name() if owner is None else (self.bus.row_id(owner) or owner)
        # One that holds the loop for 5 s is said (#678): a `def` method, or
        # a step of an `async def` one.
        label = f"{service}.{method}"

        def call() -> Any:
            value = self.blocking.held(shown, label, lambda: function(*args))
            return self.blocking.watched(shown, label, value)

        return as_row(owner, call)

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
            key, module, config, isolate, inject, exports, row = (list(args) + [None] * 7)[:7]
            labels = {name: label for name, label in (isolate or [])}
            scope = row.get("scope") if isinstance(row, dict) else None
            id = row.get("id") if isinstance(row, dict) else None
            # A module's name, or a plugin definition this runtime handed over.
            entry = module if isinstance(module, sdk.Plugin) else str(module)
            injected = inject if isinstance(inject, list) else None
            return self.load(key, entry, config, exports, labels, scope, id, injected)
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




def _module_file(module: Any) -> str | None:
    """The source file of a row's plugin: its module's, or the file of a
    plugin definition's `apply`."""
    try:
        if isinstance(module, str):
            loaded = sys.modules.get(module)
            return getattr(loaded, "__file__", None)
        return inspect.getsourcefile(module.apply)
    except (TypeError, AttributeError):
        return None


_exit_runtimes: list = []


def _exit_named(runtime: Runtime) -> None:
    """Name the row calling `os._exit`, from the runtime of the latest
    session (a listening runtime serves one session at a time)."""
    if not _exit_runtimes:
        install_exit(
            lambda key, files: _exit_runtimes[-1].name_row(key, files),
            lambda: _exit_runtimes[-1]._runtime_name(),
        )
    _exit_runtimes[:] = [runtime]


def _binding(error: BaseException) -> BindingError:
    """`error` as a BindingError, its code and data kept."""
    binding = BindingError(str(error))
    binding.code, binding.data = error.code, error.data
    return binding


def _count(n: int) -> str:
    return f"{n} argument" if n == 1 else f"{n} arguments"


def check_arguments(service: str, method: str, function: Callable, args: list) -> None:
    """Refuse a call of `service.method` with arguments `function` cannot
    take (a BindingError with the code `bad-argument`), before calling it:
    `weather.today takes 1 argument, got 2`, `weather.today: argument 1
    is missing`. A function whose signature cannot be read is called as
    it is."""
    try:
        parameters = list(inspect.signature(function).parameters.values())
    except (TypeError, ValueError):
        return
    positional = [
        p for p in parameters if p.kind in (inspect.Parameter.POSITIONAL_ONLY, inspect.Parameter.POSITIONAL_OR_KEYWORD)
    ]
    if any(p.kind is inspect.Parameter.VAR_POSITIONAL for p in parameters):
        expected = None
    else:
        expected = len(positional)
    given = len(args)
    if expected is not None and given > expected:
        error = BindingError(f"{service}.{method} takes {_count(expected)}, got {given}")
        error.code = "bad-argument"
        error.data = {"service": service, "method": method, "expected": expected, "received": given}
        raise error
    for index, parameter in enumerate(positional):
        if index >= given and parameter.default is inspect.Parameter.empty:
            annotation = parameter.annotation
            shown = None
            if annotation is not inspect.Parameter.empty:
                shown = annotation if isinstance(annotation, str) else getattr(annotation, "__name__", str(annotation))
            error = BindingError(f"{service}.{method}: argument {index + 1} is missing")
            error.code = "bad-argument"
            error.data = {"service": service, "method": method, "argument": index + 1, "expected": shown}
            raise error


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


# This package's own files: their frames are not the plugin's.
_RUNTIME_DIR = os.path.dirname(os.path.realpath(__file__)) + os.sep
# Where Python and installed packages live: the import machinery, the
# standard library, site-packages. Their frames are not the plugin's own
# code either, unless the plugin itself is there.
_NOT_THE_PLUGINS = tuple(
    dict.fromkeys(
        os.path.join(os.path.realpath(prefix), "") for prefix in (sys.prefix, sys.base_prefix, sys.exec_prefix)
    )
)


def _plugin_frame(frame: traceback.FrameSummary, own: str | None = None) -> bool:
    """Whether a frame is of the plugin's own code: not of this runtime, of
    Python's import machinery or standard library, or of an installed
    package (`site-packages`, `dist-packages`), unless it is the plugin's
    own (`own`: its package's directory, or its module's file)."""
    filename = frame.filename or ""
    if filename.startswith("<"):
        return False
    path = os.path.realpath(filename)
    if path.startswith(_RUNTIME_DIR):
        return False
    if own is not None and path.startswith(own):
        return True
    parts = path.split(os.sep)
    if "site-packages" in parts or "dist-packages" in parts:
        return False
    return not path.startswith(_NOT_THE_PLUGINS)


def _shown_path(path: str) -> str:
    """`path` relative to the working directory when it is under it."""
    try:
        near = os.path.relpath(path)
    except ValueError:
        return path
    return path if near.startswith("..") or os.path.isabs(near) else near


def cannot_load(error: BaseException, module: str) -> LoadError:
    """What importing the plugin module `module` raised, said of the file
    that raised it (a module it imports, say), not of the entry:
    `cannot load weather/util.py: KeyError: 'region' (weather/util.py:3)`.
    Its stack is the plugin's frames only."""
    every = traceback.extract_tb(error.__traceback__)
    # A plugin installed as a package lives in site-packages itself: the
    # frames of its own package are its code.
    try:
        spec = importlib.util.find_spec(module.split(".", 1)[0]) if module else None
    except Exception:  # noqa: BLE001 - only a hint
        spec = None
    origin = getattr(spec, "origin", None) if spec is not None else None
    own = None
    if isinstance(origin, str) and os.path.isabs(origin):
        own = os.path.realpath(origin)
        if getattr(spec, "submodule_search_locations", None):
            own = os.path.join(os.path.dirname(own), "")
    frames = [frame for frame in every if _plugin_frame(frame, own)]
    if isinstance(error, SyntaxError):
        text = str(error.msg)
        file, line = error.filename, error.lineno
    else:
        text = str(error)
        file, line = (frames[-1].filename, frames[-1].lineno) if frames else (None, None)
    text = text.split("\n", 1)[0]
    shown = _shown_path(file) if file and not file.startswith("<") else module
    where = f" ({shown}:{line})" if line is not None and shown != module else ""
    message = f"cannot load {shown}: {type(error).__name__}: {text}{where}"
    stack = "".join(traceback.StackSummary.from_list(frames).format())
    return LoadError(message, f"LoadError: {message}\n{stack}".rstrip("\n"))


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

"""The plugin conformance-events (#464,
docs/design-events-protocol-2026-10-18.md §13.1): each row is one
participant of an event expectation, a listener or an emitter, and logs
what it does through the host's synchronous service `events-log`.

Its config is the participant: a listener (`label`, `name`, `kind`, `log`,
`does`, `options`) or `{"label": ..., "emitter": true}`; either may name a
service it `provides` besides. An emitter provides `emitter-<label>`, whose
`dispatch(spec)` (`dispatch_sync(spec)` for `bail`) emits with the native
API and returns the result or raises.
"""

import asyncio
import time

inject = ["events-log"]

# What this process logged, so an emitter here can wait for a listener here
# (`cancel.when`).
_logged: list = []
_waiting: list = []


def _note(entry: str) -> None:
    _logged.append(entry)
    for entry_waited, future in list(_waiting):
        if entry_waited == entry and not future.done():
            future.set_result(None)


async def _wait_logged(entry: str) -> None:
    if entry in _logged:
        return
    future = asyncio.get_running_loop().create_future()
    _waiting.append((entry, future))
    try:
        await future
    finally:
        _waiting.remove((entry, future))


def text(value) -> str:
    """A logged value: as JSON writes it, numbers without a fraction as
    integers."""
    if value is None:
        return "null"
    if value is True or value is False:
        return "true" if value else "false"
    if isinstance(value, float) and value.is_integer():
        return str(int(value))
    return str(value)


def _thrown(spec: dict) -> Exception:
    return type(spec.get("name", "Error"), (Exception,), {})(spec.get("message", ""))


# What `does` needs to wait for: a function of kind `any` returns an
# awaitable for these.
_WAITING = ("after", "next", "nextTwice", "waits", "emits")


class Present:
    """The service a row provides with `provides`."""

    def ping(self):
        return "pong"


class Row:
    def __init__(self, ctx, participant: dict):
        self.ctx = ctx
        self.p = participant
        self.label = participant["label"]
        self.log = ctx.use("events-log")

    def add(self, entry: str) -> None:
        _note(entry)
        self.log.add(entry)

    # ── Emitting ─────────────────────────────────────────────────

    def emitter(self, spec: dict):
        chosen = spec.get("filter") or {}
        if "throws" in chosen:
            def throws(_descriptor):
                raise _thrown(chosen["throws"])

            return self.ctx.filter(throws)
        keep = chosen.get("keep")
        if keep is None:
            return self.ctx

        def predicate(descriptor):
            self.add(f"filter:{descriptor['row']}")
            return descriptor["row"] in keep

        return self.ctx.filter(predicate)

    def dispatch_sync(self, spec: dict):
        emitter, name, args = self.emitter(spec), spec.get("name") or self.p.get("name"), list(spec.get("args") or [])
        if spec["mode"] == "emit":
            emitter.emit(name, *args)
            return None
        if spec["mode"] != "bail":
            raise ValueError(f"conformance-events: {spec['mode']} is not synchronous")
        return emitter.bail(name, *args)

    async def dispatch(self, spec: dict):
        mode = spec["mode"]
        if mode in ("emit", "bail"):
            return self.dispatch_sync(spec)
        emitter, name, args = self.emitter(spec), spec.get("name") or self.p.get("name"), list(spec.get("args") or [])
        if mode == "parallel":
            dispatched = emitter.parallel(name, *args)
        elif mode == "serial":
            dispatched = emitter.serial(name, *args)
        elif mode == "waterfall":
            terminal = None
            if spec.get("terminal") is not None:
                end = spec["terminal"]

                # A text it returns, or {"echo": true}: its first argument.
                def terminal(*args):
                    self.add("terminal")
                    return args[0] if isinstance(end, dict) and end.get("echo") else end

            dispatched = emitter.waterfall(name, *args, terminal=terminal)
        else:
            raise ValueError(f"conformance-events: mode {mode} is not Python's to emit")
        cancel = spec.get("cancel")
        if cancel is None:
            return await dispatched
        task = asyncio.ensure_future(dispatched)
        waiting = asyncio.ensure_future(_wait_logged(cancel["when"]))
        await asyncio.wait({task, waiting}, return_when=asyncio.FIRST_COMPLETED)
        waiting.cancel()
        if not task.done():
            task.cancel()
        try:
            return await task
        except asyncio.CancelledError:
            raise RuntimeError("the emitter cancelled the dispatch") from None

    # ── Listening ────────────────────────────────────────────────

    def entry(self, args: list):
        log = self.p.get("log", "label")
        if log is False:
            return None
        if log == "label" or log is None:
            return self.label
        first = args[0] if args else None
        if log == "arg":
            return f"{self.label}:{text(first)}"
        record = first if isinstance(first, dict) else {}
        return f"{self.label}:" + ",".join(text(record.get(field)) for field in log)

    def listener(self):
        kind, does = self.p.get("kind", "async"), self.p["does"]

        def start(args):
            entry = self.entry(list(args))
            if entry is not None:
                self.add(entry)

        if kind == "async":

            async def listener(*args):
                start(args)
                return await self.does_async(does, list(args))

        elif kind == "any":

            def listener(*args):
                start(args)
                if any(key in does for key in _WAITING):
                    return self.does_async(does, list(args))
                return self.does_sync(does, list(args))

        else:

            def listener(*args):
                start(args)
                return self.does_sync(does, list(args))

        return listener

    def does_sync(self, does: dict, args: list):
        if "returns" in does:
            return does["returns"]
        if "returnsNothing" in does:
            return None
        if "echo" in does:
            return args[0] if args else None
        if "throws" in does:
            raise _thrown(does["throws"])
        if "veto" in does:
            return does["veto"]
        if "calls" in does:
            return args[does["calls"]]("x")
        if "after" in does:
            time.sleep(does["after"] / 1000)
            return self.does_sync(does["then"], args)
        if "next" in does:
            rest = args[-1]()
            return does["next"] + ("" if rest is None or rest is False else text(rest))
        if "nextTwice" in does:
            args[-1]()
            return args[-1]()
        raise ValueError(f"conformance-events: {does} cannot be synchronous")

    async def does_async(self, does: dict, args: list):
        if "after" in does:
            await asyncio.sleep(does["after"] / 1000)
            return await self.does_async(does["then"], args)
        if "next" in does:
            rest = await args[-1]()
            return does["next"] + ("" if rest is None or rest is False else text(rest))
        if "nextTwice" in does:
            await args[-1]()
            return await args[-1]()
        if "waits" in does:
            try:
                await asyncio.get_running_loop().create_future()
            except asyncio.CancelledError:
                self.add(f"{self.label}:cancelled")
                return None
        if "emits" in does:
            return await self.dispatch(does["emits"])
        if "calls" in does:
            called = args[does["calls"]]("x")
            return await called if asyncio.iscoroutine(called) or asyncio.isfuture(called) else called
        return self.does_sync(does, args)


class Emitter:
    """The service emitter-<label>."""

    def __init__(self, row: Row):
        self._row = row

    async def dispatch(self, spec):
        return await self._row.dispatch(spec)

    def dispatch_sync(self, spec):
        return self._row.dispatch_sync(spec)


def apply(ctx, participant):
    row = Row(ctx, participant)
    if participant.get("emitter"):
        ctx.provide(f"emitter-{row.label}", Emitter(row))
    else:
        options = participant.get("options") or {}
        ctx.on(
            participant["name"],
            row.listener(),
            prepend=bool(options.get("prepend")),
            global_=bool(options.get("global")),
        )
    if participant.get("provides"):
        ctx.provide(participant["provides"], Present())

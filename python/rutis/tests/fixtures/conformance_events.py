"""The `conformance-events` plugin for the Python runtime (#464 step 7).

Every participant of a case in
crates/rutis-bridge/src/runtime/testing/events.expected.json is one row of
this plugin, keyed by its label; the row's config is the participant as the
runner resolved it (name, kind and log filled in). What a row does is
described in that file's `about` and in
crates/rutis-bridge/src/runtime/testing/events.rs. The real runtime loads it
as `py:conformance_events` (listener rows) and `py:conformance_events_e` /
`py:conformance_events_f` (emitter rows) with this directory as the
project; the testing
tool loads it in tests/test_events_conformance.py.

A row with `name` registers one listener (`ctx.on(name, fn, prepend=...,
global_=...)`) that logs its start to the host service `events-log` and then
does what `does` says. A row with `emitter` provides `emitter-<label>`,
whose `dispatch(spec)` / `dispatch_sync(spec)` emit with the native API.
"""

import asyncio
import inspect
import json
import time

from rutis import define_plugin

inject = ["events-log"]

# The shape of `emitter-<label>`. This module declares no `provides` (a
# declared service is exported by every row of the module): emitter rows
# load conformance_events_e / conformance_events_f, the same plugin
# declaring only its own `emitter-e` / `emitter-f`.
EMITTER = {"dispatch": "async", "dispatch_sync": "sync"}

# What a listener of kind `any` cannot do without returning an awaitable.
_WAITING = {"after", "next", "nextTwice", "waits", "calls", "emits"}


def fmt(value):
    """A value as the log writes it: strings as they are, everything else
    as JSON (`1`, `true`, `null`)."""
    if isinstance(value, str):
        return value
    if isinstance(value, float) and value.is_integer():
        return str(int(value))
    try:
        return json.dumps(value, separators=(",", ":"))
    except TypeError:
        return repr(value)


def error(spec):
    """An exception of the name and message `spec` gives."""
    return type(spec["name"], (Exception,), {})(spec.get("message", ""))


def _field(value, name):
    return value.get(name) if isinstance(value, dict) else getattr(value, name, None)


def _rest(text, result):
    return text + ("" if result is None or result is False else str(result))


def dispatch(ctx, log, spec):
    """Emit `spec` from `ctx`'s row: the value for `emit` (None) and `bail`,
    a coroutine for the others."""
    name, mode, args = spec["name"], spec["mode"], list(spec.get("args", []))
    emitter = ctx
    chosen = spec.get("filter")
    if chosen is not None:
        if "throws" in chosen:
            def predicate(description):
                raise error(chosen["throws"])
        else:
            def predicate(description):
                log.filter(description)
                return description["row"] in chosen["keep"]
        emitter = ctx.filter(predicate)
    if mode == "emit":
        emitter.emit(name, *args)
        return None
    if mode == "bail":
        return emitter.bail(name, *args)
    terminal = None
    if isinstance(spec.get("terminal"), str):
        def terminal(*_args):
            log.add("terminal")
            return spec["terminal"]
    elif isinstance(spec.get("terminal"), dict) and spec["terminal"].get("echo"):
        # {"echo": true}: it returns its first argument.
        def terminal(*args):
            log.add("terminal")
            return args[0]

    async def run():
        if mode == "parallel":
            await emitter.parallel(name, *args)
            return None
        if mode == "serial":
            return await emitter.serial(name, *args)
        if mode == "waterfall":
            return await emitter.waterfall(name, *args, terminal=terminal)
        raise ValueError(f"the Python emitter has no mode {mode}")

    return run()


class Emitter:
    """The service `emitter-<label>`."""

    def __init__(self, ctx, log):
        self._ctx = ctx
        self._log = log

    async def dispatch(self, spec):
        work = dispatch(self._ctx, self._log, spec)
        if not asyncio.iscoroutine(work):
            return work
        task = asyncio.ensure_future(work)
        cancel = spec.get("cancel")
        if cancel is not None:
            # Give up the dispatch once that listener has started; the
            # CancelledError is the dispatch's outcome.
            started = asyncio.ensure_future(self._log.wait(cancel["when"]))
            await asyncio.wait({task, started}, return_when=asyncio.FIRST_COMPLETED)
            if started.done():
                task.cancel()
            else:
                started.cancel()
        return await task

    def dispatch_sync(self, spec):
        work = dispatch(self._ctx, self._log, spec)
        if asyncio.iscoroutine(work):
            work.close()
            raise ValueError(f"dispatch_sync is for bail, not {spec['mode']}")
        return work


class Service:
    """An undeclared service a row provides (`provides` in its config)."""

    def ping(self):
        return None


def listener(ctx, log, config):
    """The one listener of a listener row."""
    label, does, kind = config["label"], config["does"], config.get("kind", "async")
    how = config.get("log", "label")

    def started(args):
        if how is False:
            return
        if how == "label":
            log.add(label)
        elif how == "arg":
            log.add(f"{label}:{fmt(args[0])}")
        else:
            log.add(f"{label}:" + ",".join(fmt(_field(args[0], field)) for field in how))

    def now(does, args):
        # `does` done synchronously; `next` is the last argument.
        if "returns" in does:
            return does["returns"]
        if "returnsNothing" in does:
            return None
        if "echo" in does:
            return args[0]
        if "throws" in does:
            raise error(does["throws"])
        if "after" in does:
            time.sleep(does["after"] / 1000)
            return now(does["then"], args)
        if "next" in does:
            return _rest(does["next"], args[-1]())
        if "veto" in does:
            return does["veto"]
        if "nextTwice" in does:
            args[-1]()
            return args[-1]()
        if "calls" in does:
            return args[does["calls"]]("x")
        if "emits" in does:
            return dispatch(ctx, log, does["emits"])
        raise ValueError(f"a synchronous Python listener cannot do {does}")

    async def later(does, args):
        # `does` done asynchronously, awaiting what it waits for.
        if "after" in does:
            await asyncio.sleep(does["after"] / 1000)
            return await later(does["then"], args)
        if "next" in does:
            return _rest(does["next"], await _settled(args[-1]()))
        if "nextTwice" in does:
            await _settled(args[-1]())
            return await _settled(args[-1]())
        if "waits" in does:
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                log.add(f"{label}:cancelled")
            return None
        if "calls" in does:
            return await _settled(args[does["calls"]]("x"))
        if "emits" in does:
            return await _settled(dispatch(ctx, log, does["emits"]))
        return now(does, args)

    if kind == "async":
        async def on_event(*args):
            started(args)
            return await later(does, args)
    elif kind == "any":
        def on_event(*args):
            started(args)
            # A plain function: a coroutine only when `does` must wait.
            return later(does, args) if _WAITING & does.keys() else now(does, args)
    else:
        def on_event(*args):
            started(args)
            return now(does, args)

    return on_event


async def _settled(value):
    while inspect.isawaitable(value):
        value = await value
    return value


def apply(ctx, config):
    log = ctx.use("events-log")
    if "fails" in config:
        raise error(config["fails"])
    if config.get("emitter"):
        ctx.provide(f"emitter-{config['label']}", Emitter(ctx, log))
    if config.get("provides"):
        ctx.provide(config["provides"], Service())
    if "name" in config:
        options = config.get("options") or {}
        ctx.on(
            config["name"],
            listener(ctx, log, config),
            prepend=bool(options.get("prepend")),
            global_=bool(options.get("global")),
        )


# Declared with its name, as the other languages name it: the descriptors
# filters see call it `conformance-events` (without a name, a Python
# plugin is called by its row's id).
plugin = define_plugin(apply, inject=inject, name="conformance-events")

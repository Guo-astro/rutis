"""One session of the rutis protocol (version 4), as the runtime side.

This is `node/rutis-runtime/src/peer.mjs` in Python. The asyncio loop thread owns
every table and runs all plugin code; a reader thread only parses frames and
hands them over. A synchronous call made on the loop thread blocks it and
pumps incoming frames itself: invocations belonging to its call chain (their
`path` names a call it waits for) run inline on the loop thread.

Unlike the Node runtime, unrelated invocations run inline too (`reentrant`).
Two runtimes that synchronously call each other's services at the same time
would otherwise each wait for the other forever: each would hold back the
other's call until its own returns. So a plugin's service may be called
while that plugin is itself inside a synchronous call; do not hold a lock
across a call into rutis.
"""

from __future__ import annotations

import asyncio
import inspect
import collections
import contextvars
import dataclasses
import datetime as _dt
import decimal
import json
import re
import socket
import sys
import threading
import traceback
import uuid
import weakref
from typing import Any, Callable

from . import _eager, _values
from .plugin import Plugin
from .channel import MAX_MESSAGE, SocketChannel

# The session protocol: endpoint ids in the handshake and the call ids,
# capabilities, either side calling the other.
ENDPOINT_PROTOCOL = 4
# What this implementation supports. `objects`: it
# takes objects with methods, as a proxy whose methods call them (#687). `sync-wait`: its
# synchronous calls say so (`sync: true`), for the host's wait-cycle check;
# `sync-stack`: calls run while it waits are stacked on its one thread;
# `value-<tag>`: it takes every tagged value (#308, `rutis._values`);
# `events-v2`: it speaks the event protocol of rutis 0.13 (#464), which the
# runtime requires of its host too (`require`).
CAPABILITIES = ["objects", "signals", "reentrant-sync", "sync-wait", "sync-stack", "events-v2", *_values.CAPABILITIES]
# Why a runtime ends a session with a host that does not declare
# `events-v2` (docs/design-events-protocol-2026-10-18.md §2.2).
EVENTS_REFUSAL = (
    "this host does not declare events-v2 (the event protocol of rutis 0.13): "
    "upgrade rutis-host to 0.13 or later"
)
IMPLEMENTATION = {"name": "rutis", "version": "0.18.0"}


def version_mismatch(far, local: int, implementation) -> str:
    """Why a far end's protocol version is refused, saying which end to
    upgrade (standard Q6.2.4): this side when the far end is newer, naming
    the package to upgrade here; the far end when it is older, naming its
    implementation when its handshake said."""
    mismatch = f"the far end speaks protocol {far}, this side {local}"
    if not isinstance(far, int) or isinstance(far, bool) or far == local:
        return mismatch
    if far > local:
        return f"{mismatch}: upgrade this side (PyPI rutis)"
    if isinstance(implementation, dict):
        name, version = implementation.get("name"), implementation.get("version")
        if isinstance(name, str) and isinstance(version, str):
            return f"{mismatch}: upgrade the far end ({name} {version})"
    return f"{mismatch}: upgrade the far end"

MAX_SAFE = 9_007_199_254_740_991
_ENDPOINT_ID = re.compile(r"^[a-z0-9-]+$")
# An id in a chain: either side's, possibly tagged with the session it came
# through (`s3/mac:4`).
_ENDPOINT_ORIGIN = re.compile(r"^(s[0-9]+/)?[a-z0-9-]+:[1-9][0-9]*$")
_SEQUENCE = re.compile(r"^[1-9][0-9]*$")


class _Undefined:
    """The wire's `undefined`: an omitted argument or field."""

    _instance = None

    def __new__(cls):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __repr__(self) -> str:
        # What a plugin sees of it, should it ever reach one: it stands for
        # an omitted value, which Python writes None.
        return "None"

    def __bool__(self) -> bool:
        return False


UNDEFINED = _Undefined()
# Undefined inside data, as sent (design V10).
_NULL = {"type": "data", "value": None}


class RemoteError(Exception):
    """An error thrown on the other side of the session."""

    def __init__(self, name: str, message: str, graph: Any = None):
        super().__init__(message)
        self.name = name
        self.message = message
        self.graph = graph
        # What a Rust service's `Err(e)` carries (#440): its code (the
        # variant's name) and data (`e`'s serde value); None otherwise.
        self.code, self.data = _code_and_data(graph)

    def __str__(self) -> str:
        return f"{self.name}: {self.message}"


def _code_and_data(graph: Any) -> tuple[str | None, Any]:
    """The `code` and `data` of the error a graph's root refers to."""
    try:
        nodes = graph["nodes"]
        root = nodes[graph["root"]["value"]]
        if root.get("type") != "error":
            return None, None
        code = root.get("code")
        code = code if isinstance(code, str) else None
    except (KeyError, IndexError, TypeError, ValueError, AttributeError):
        return None, None

    def read(value, depth=0):
        if depth > 64:
            raise ValueError("error data nested too deep")
        kind = value["type"]
        if kind == "data":
            return value["value"]
        if kind == "undefined":
            return None
        if kind == "number":
            return float(value["value"])
        if kind == "map":
            entries = value["value"]
            return _values.received_map([read(k, depth + 1) for k, _ in entries], [read(v, depth + 1) for _, v in entries])
        if kind == "set":
            return _values.received_set([read(item, depth + 1) for item in value["value"]])
        # The other tagged values, as the session decodes them (bigint included).
        if kind in _values.TAGS:
            return _values.read(kind, value["value"])
        node = nodes[value["value"]]
        if node["type"] == "array":
            return [read(item, depth + 1) for item in node["values"]]
        if node["type"] == "object":
            return {key: read(item, depth + 1) for key, item in node["values"]}
        raise ValueError("unexpected error data")

    # Data that cannot be read leaves the code as it is.
    try:
        data = read(root["data"]) if "data" in root else None
    except (KeyError, IndexError, TypeError, ValueError, AttributeError, _values.Refused):
        data = None
    return code, data


def _graph_data(value: Any, nodes: list, depth: int = 0) -> dict:
    """`value` as an error graph value (an exception's `data`): plain data,
    big integers as bigints, lists and dicts as nodes. Anything else raises
    ValueError, and the data is left out."""
    if depth > 64:
        raise ValueError("error data nested too deep")
    if value is None or isinstance(value, (bool, str)):
        return {"type": "data", "value": value}
    if isinstance(value, int):
        if abs(value) > 2**53 - 1:
            return {"type": "bigint", "value": str(value)}
        return {"type": "data", "value": value}
    if isinstance(value, float):
        if value != value or value in (float("inf"), float("-inf")):
            return {"type": "number", "value": "NaN" if value != value else ("Infinity" if value > 0 else "-Infinity")}
        return {"type": "data", "value": value}
    if isinstance(value, (list, tuple)):
        node = {"type": "array", "values": []}
        nodes.append(node)
        reference = {"type": "reference", "value": len(nodes) - 1}
        node["values"] = [_graph_data(item, nodes, depth + 1) for item in value]
        return reference
    if isinstance(value, dict) and all(isinstance(key, str) for key in value):
        node = {"type": "object", "values": []}
        nodes.append(node)
        reference = {"type": "reference", "value": len(nodes) - 1}
        node["values"] = [[key, _graph_data(item, nodes, depth + 1)] for key, item in value.items()]
        return reference
    raise ValueError("error data that cannot cross")


class LoadError(Exception):
    """A plugin module that cannot be loaded: importing it, or a module it
    imports, raised. The message says which file raised, what and where
    (`cannot load weather/util.py: KeyError: 'region' (weather/util.py:3)`);
    `stack` is the traceback of the plugin's own frames, without the
    runtime's (the diagnostics design §8.1, row 5)."""

    def __init__(self, message: str, stack: str = ""):
        super().__init__(message)
        self.message = message
        self.stack = stack or f"LoadError: {message}"


class SyncWaitCycle(RemoteError):
    def __init__(self, message: str):
        super().__init__("SyncWaitCycle", message)

    def __str__(self) -> str:
        # The class says what it is: the message alone, as it was sent.
        return self.message


class Incompatible(ValueError):
    """The far end greeted, but this side cannot work with it: it lacks a
    capability this side requires (`events-v2`). Retrying cannot fix it."""


class _Named(RemoteError):
    """An error of rutis's own with a fixed name, raised on either side of
    a session and decoded back into its class."""

    NAME = ""

    def __init__(self, message: str, graph: Any = None):
        super().__init__(self.NAME, message, graph)

    def __str__(self) -> str:
        # The class says what it is.
        return self.message


class BindingError(_Named):
    """A call that cannot reach what it calls: a withdrawn listener, a
    refused frame, a transport failure."""

    NAME = "BindingError"


class AsyncListener(_Named):
    """A synchronous `bail` met an asynchronous listener: an `async def`,
    or a function that returned an awaitable."""

    NAME = "AsyncListener"


class WaterfallNext(_Named):
    """A waterfall's `next` called twice, or after its waterfall ended."""

    NAME = "WaterfallNext"


class ReservedEvent(_Named):
    """A plugin emitted a `rutis/*` event: only rutis emits those."""

    NAME = "ReservedEvent"


_NAMED = {named.NAME: named for named in (BindingError, AsyncListener, WaterfallNext, ReservedEvent)}


class AggregateError(Exception):
    """Ordered listener failures, also on Python 3.10 (no ExceptionGroup):
    what `parallel` raises when any listener failed, with the failures in
    `errors` in registration order (also when just one failed). It crosses
    as Cordis's AggregateError, its children in `errors`; one decoded from
    the far end is an AggregateError again, its children RemoteErrors."""

    name = "AggregateError"

    def __init__(self, errors, message: str = ""):
        super().__init__(message)
        self.errors = list(errors)
        self.message = message

    def __str__(self) -> str:
        return self.message or f"multiple errors: {self.errors!r}"


class Signal:
    """Cancellation of the call that received it: set when the caller
    cancels. Methods that accept it check `cancelled` or await `wait()`."""

    def __init__(self) -> None:
        self.cancelled = False
        self._event = asyncio.Event()

    def _cancel(self) -> None:
        self.cancelled = True
        self._event.set()

    async def wait(self) -> None:
        await self._event.wait()


def _message(error: BaseException) -> str:
    return error.message if isinstance(error, AggregateError) else str(error)


def encode_error(thrown: BaseException) -> dict:
    """The wire form of `thrown`: its error graph (an AggregateError's
    children in order, shared and cyclic nodes once). A RemoteError thrown
    on, inside an aggregate or alone, crosses with the far end's graph
    spliced in (nodes appended, references shifted), as Node does: it
    decodes into a real Error whose re-encoding carries the whole graph."""
    if isinstance(thrown, SyncWaitCycle):
        return {"name": "SyncWaitCycle", "message": thrown.message}
    if isinstance(thrown, RemoteError):
        name, message = thrown.name, thrown.message
    else:
        name, message = type(thrown).__name__, _message(thrown)
    nodes, seen = [], {}

    def visit(error):
        if id(error) in seen:
            return seen[id(error)]
        offset = len(nodes)
        reference = {"type": "reference", "value": offset}
        seen[id(error)] = reference
        graph = _well_formed(error.graph) if isinstance(error, RemoteError) else None
        if graph is not None:
            def shift(value):
                if isinstance(value, list):
                    return [shift(item) for item in value]
                if isinstance(value, dict):
                    if value.get("type") == "reference":
                        return {"type": "reference", "value": value["value"] + offset}
                    return {key: shift(item) for key, item in value.items()}
                return value

            nodes.extend(shift(graph["nodes"]))
            reference = shift(graph["root"])
            seen[id(error)] = reference
            _append_rethrown(nodes, reference, error)
            return reference
        node = {
            "type": "error",
            "name": error.name if isinstance(error, RemoteError) else type(error).__name__,
            "message": error.message if isinstance(error, RemoteError) else _message(error),
            # An error whose cause is itself the error it reports (an
            # `UnhandledError`) leaves the chain to its `cause` node, so
            # the same traceback is not in its stack twice.
            "stack": error.stack
            if isinstance(error, LoadError)
            else "".join(traceback.format_exception(error, chain=not getattr(error, "reports_its_cause", False))),
        }
        nodes.append(node)
        # A str `code` and its `data` cross, as from JavaScript and Rust
        # (#440); data that cannot cross is left out, nothing of it kept.
        code = getattr(error, "code", None)
        if isinstance(code, str):
            node["code"] = code
        if hasattr(error, "data") and not (isinstance(error, RemoteError) and error.data is None):
            mark = len(nodes)
            try:
                node["data"] = _graph_data(error.data, nodes)
            except (ValueError, RecursionError):
                del nodes[mark:]
        if isinstance(error, AggregateError):
            node["errors"] = [visit(item) for item in error.errors]
        # `raise … from cause`: the cause crosses as its own node, as a
        # JavaScript error's `cause` does.
        if isinstance(error.__cause__, BaseException):
            node["cause"] = visit(error.__cause__)
        return reference

    root = visit(thrown)
    return {
        "name": name,
        "message": message,
        "graph": {"root": root, "nodes": nodes},
    }


_MAX_RETHROWN = 8


def _append_rethrown(nodes: list, reference: Any, error: BaseException) -> None:
    """A received error thrown on: its spliced root node gets this
    process's frames for the rethrow as one more `rethrown` item
    (design 2026-10-26 §4.4), at most 8; the received graph is kept as
    it came (the node is replaced by a copy)."""
    if error.__traceback__ is None:
        return
    if not isinstance(reference, dict) or reference.get("type") != "reference":
        return
    index = reference.get("value")
    if not isinstance(index, int) or not 0 <= index < len(nodes):
        return
    root = nodes[index]
    if not isinstance(root, dict) or root.get("type") != "error":
        return
    rethrown = root.get("rethrown")
    if rethrown is None:
        rethrown = []
    if not isinstance(rethrown, list) or len(rethrown) >= _MAX_RETHROWN:
        return
    frames = "".join(traceback.format_tb(error.__traceback__)).rstrip("\n")
    if not frames:
        return
    nodes[index] = {**root, "rethrown": [*rethrown, frames]}


def _well_formed(graph: Any) -> dict | None:
    """An error graph as relayed: its root and its nodes (an array, also
    when a far end sent none or null); None when it is not one, and the
    error crosses on with its name and message alone (#675)."""
    if not isinstance(graph, dict) or not isinstance(graph.get("root"), dict):
        return None
    nodes = graph.get("nodes")
    if nodes is None:
        nodes = []
    if not isinstance(nodes, list) or not all(isinstance(node, dict) for node in nodes):
        return None
    return {"root": graph["root"], "nodes": nodes}


def decode_error(failure: dict, depth: int = 0) -> Exception:
    """The error the far end threw: a RemoteError (rutis's own named errors
    as their classes), or an AggregateError with its children."""
    name = str(failure.get("name", "Error"))
    message = str(failure.get("message", ""))
    graph = failure.get("graph")
    if name == "SyncWaitCycle" and graph is None:
        return SyncWaitCycle(message)
    if name == "AggregateError":
        return AggregateError(_children(graph, depth), message)
    if name in _NAMED:
        return _NAMED[name](message, graph)
    return RemoteError(name, message, graph)


def _children(graph: Any, depth: int) -> list:
    """The errors of the aggregate a graph's root is, each with the graph
    rooted at it; none when the graph does not say."""
    if depth > MAX_DEPTH:
        return []
    try:
        nodes = graph["nodes"]
        references = nodes[graph["root"]["value"]].get("errors") or []
        children = []
        for reference in references:
            node = nodes[reference["value"]]
            failure = {"name": node.get("name", "Error"), "message": node.get("message", "")}
            failure["graph"] = {"root": reference, "nodes": nodes}
            children.append(decode_error(failure, depth + 1))
        return children
    except (KeyError, IndexError, TypeError, AttributeError):
        return []


def dumps(frame: dict) -> bytes:
    """Compact JSON; newlines inside strings are escaped, so a frame never
    contains a raw one. The channel frames it."""
    return json.dumps(frame, separators=(",", ":"), allow_nan=False).encode()


_DATA = (type(None), bool, int, float, str)
# Values of their own kind (#308): they cross by value under their tag, not
# as references (#355).
_VALUES = (_dt.date, _dt.time, decimal.Decimal, uuid.UUID, memoryview)


def _is_future(value: Any) -> bool:
    return asyncio.isfuture(value) or asyncio.iscoroutine(value)


def _is_live(value: Any) -> bool:
    """An object with behaviour, which crosses by reference."""
    if value is UNDEFINED:
        return False
    if isinstance(value, _DATA + _VALUES + (list, tuple, dict, bytes, bytearray, set, frozenset)):
        return False
    if isinstance(value, BaseException) or dataclasses.is_dataclass(value):
        return False
    if isinstance(value, type) or callable(value) or _is_future(value):
        return False
    return True


def _holds_reference(value: Any, seen: set | None = None, depth: int = 0) -> bool:
    """Whether `value` holds a reference. Not looked for past MAX_DEPTH
    levels: a value that deep fails as data, too deep (#279)."""
    if isinstance(value, (RemoteFunction, RemoteFuture, RemoteObject, Plugin)):
        return True
    if callable(value) or _is_future(value) or _is_live(value):
        return True
    if isinstance(value, (list, tuple, dict)):
        seen = seen if seen is not None else set()
        if id(value) in seen or depth > MAX_DEPTH:
            return False
        seen.add(id(value))
        items = value.values() if isinstance(value, dict) else value
        return any(_holds_reference(item, seen, depth + 1) for item in items)
    return False


def _beyond_data(value: Any, seen: set | None = None, depth: int = 0) -> bool:
    """Whether `value` holds a reference or a tagged value, so it crosses
    as a list or record of tagged parts, not as JSON data. Not looked for
    past MAX_DEPTH levels, as `_holds_reference`."""
    if _values.tag_of(value) is not None:
        return True
    if isinstance(value, (RemoteFunction, RemoteFuture, RemoteObject)):
        return True
    if callable(value) or _is_future(value) or _is_live(value):
        return True
    if dataclasses.is_dataclass(value):
        items: Any = [getattr(value, field.name) for field in dataclasses.fields(value)]
    elif isinstance(value, (list, tuple, dict)):
        items = value.values() if isinstance(value, dict) else value
    else:
        return False
    seen = seen if seen is not None else set()
    if id(value) in seen or depth > MAX_DEPTH:
        return False
    seen.add(id(value))
    return any(_beyond_data(item, seen, depth + 1) for item in items)


# How deep a value crossing a session may nest (#279): each list, tuple,
# dict, map or set (or dataclass) is a level, so 64 nested lists cross and
# 65 do not; a call's argument list is not a level. A deeper value fails its
# own call here, before it is sent; the session goes on.
MAX_DEPTH = 64


def _too_deep() -> ValueError:
    return ValueError(
        f"value nested deeper than {MAX_DEPTH} levels, the most a value crossing processes may nest"
    )


# The largest plain integer crossing a session (#356): ±(2^53−1), the
# integers a JavaScript number holds exactly and the limit Go keeps. Past
# 2^64 Rust would read one as a float. A larger one fails its own call here,
# before it is sent; the session goes on. `bool` is an int and crosses as
# before. Beyond it, an int crosses as a bigint instead (#308, design V6):
# only an int inside data that is not looked into (deeper than MAX_DEPTH)
# reaches this limit.
MAX_SAFE_INTEGER = _values.MAX_SAFE_INTEGER
_INF = float("inf")


def _too_large() -> ValueError:
    # Without the integer: a very long one cannot even be written (Python
    # limits int-to-str conversion).
    return ValueError(
        "integer beyond ±(2^53-1), the largest an integer crossing processes as data may be"
    )


# What a call cannot send (#675): a value the far end cannot take, or a
# message over the channel's limit, fails only that call, on the side that
# would send it, and the error says which call, which argument (or the
# result) and why. The wording is rutis-bridge's
# (crates/rutis-bridge/src/session/rpc/crossing.rs).
_MIB = 1024 * 1024


def _invocation(target: Any, method: Any) -> str:
    """What a call is, for messages: `service.method` (the target without a
    `host:` prefix or a scope label), or the method alone for a control
    call (no target)."""
    service = str(target)
    if service.startswith("host:"):
        service = service[len("host:"):]
    service = service.split("\0", 1)[0]
    return f"{service}.{method}" if service else str(method)


def _described(op: str, fields: dict) -> str:
    """The call an outgoing or incoming frame makes, for messages."""
    if op == "invoke":
        return _invocation(fields.get("target"), fields.get("method"))
    if op == "call":
        return "a function" if fields.get("method") is None else f"the method {fields['method']}"
    if op == "get":
        return f"the property {fields.get('property')}"
    return "an awaited result"


BAD_VALUE = "bad-value"
PEER_GONE = "peer-gone"


def _coded(error: BaseException, code: str, data: Any) -> BaseException:
    """`error` carrying `code` and `data`, as rutis's own errors do (the
    diagnostics design §4.3): they cross with it, and a program decides
    by the code, never by the text."""
    error.code = code
    error.data = data
    return error


def _cannot_send(
    error: BaseException,
    call: str | None,
    part: str,
    index: int | None = None,
    limit: int | None = None,
    what: str | None = None,
) -> BaseException:
    """Why `call`'s argument `index` (0-based; `part` "argument") or its
    result (`part` "result") cannot be sent, `error` saying why: `svc.m:
    argument 1 cannot be sent: <why>`, as rutis-bridge says it, of
    `error`'s own class where it can be made from a message alone, with the
    code `bad-value` and `{part, argument?, limit?}`. `what` names the part
    otherwise (`the arguments`, `the error KeyError`)."""
    if what is None:
        what = f"argument {index + 1}" if index is not None else f"the {part}"
    message = f"{what} cannot be sent: {error}"
    if call:
        message = f"{call}: {message}"
    try:
        made = type(error)(message)
    except Exception:  # noqa: BLE001 - a class that takes something else
        made = ValueError(message)
    data: dict = {"part": part}
    if index is not None:
        data["argument"] = index + 1
    if limit is not None:
        data["limit"] = limit
    return _coded(made, BAD_VALUE, data)


def host_gone(message: str = "the host disconnected", reason: str = "disconnected") -> ConnectionError:
    """The process lost the host: the session's end, which every call
    still waiting fails with (code `peer-gone`)."""
    return _coded(ConnectionError(message), PEER_GONE, {"reason": reason})


def _not_finite(value: float) -> ValueError:
    """Why a float that is not a finite number cannot be sent."""
    shown = "NaN" if value != value else "Infinity" if value > 0 else "-Infinity"
    return ValueError(f"{shown} cannot cross processes")


def _over_limit(size: int, limit: int) -> str:
    """Why a message of `size` bytes is not sent over a channel taking at
    most `limit`."""
    mib = f" ({limit // _MIB} MiB)" if limit % _MIB == 0 else ""
    return f"the message would be {size} bytes, over the limit of {limit} bytes{mib}"


def _largest(args: Any) -> int | None:
    """The largest of a call's encoded arguments (a list): the one a
    message over the limit is blamed on."""
    items = args.get("value") if isinstance(args, dict) and args.get("type") in ("list", "data") else None
    if not isinstance(items, list) or not items:
        return None
    sizes = [len(dumps(item)) for item in items]
    return max(range(len(sizes)), key=sizes.__getitem__)


def _data(value: Any, depth: int = 0) -> Any:
    """Plain data for the wire, or TypeError (ValueError if nested deeper
    than MAX_DEPTH or an integer beyond ±MAX_SAFE_INTEGER; `depth` counts
    the levels around it). A dict's undefined value is None (#355)."""
    if isinstance(value, _DATA):
        if isinstance(value, int) and not -MAX_SAFE_INTEGER <= value <= MAX_SAFE_INTEGER:
            raise _too_large()
        if isinstance(value, float) and not -_INF < value < _INF:
            raise _not_finite(value)
        return value
    if isinstance(value, (list, tuple, dict)) and depth >= MAX_DEPTH:
        raise _too_deep()
    if isinstance(value, (list, tuple)):
        return [_data(item, depth + 1) for item in value]
    if isinstance(value, dict):
        if not all(isinstance(key, str) for key in value):
            raise TypeError("only string keys cross the boundary")
        return {key: _data(item, depth + 1) for key, item in value.items()}
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        try:
            fields = dataclasses.asdict(value)
        except RecursionError:
            # asdict recurses with no limit of its own.
            raise _too_deep() from None
        return _data(fields, depth)
    if value is UNDEFINED:
        return None
    raise TypeError(f"{type(value).__name__} cannot cross the boundary")


class RemoteFunction:
    """A function of the other side. Calling it is synchronous; `call_async`
    returns its result without blocking the loop."""

    def __init__(self, peer: "Peer", record: "_Import"):
        self._peer = peer
        self._record = record

    def __call__(self, *args: Any) -> Any:
        self._record.check()
        return _plain(self._peer._call_sync("call", {"reference": self._record.id}, list(args)))

    async def call_async(self, *args: Any) -> Any:
        self._record.check()
        return _plain(await self._peer._request_async("call", {"reference": self._record.id}, list(args)))

    def __repr__(self) -> str:
        return "<function from the host>"


class RemoteFuture:
    """An async result of the other side; await it (once or many times)."""

    def __init__(self, peer: "Peer", record: "_Import"):
        self._peer = peer
        self._record = record
        self._task: asyncio.Future | None = None

    def __await__(self):
        if self._task is None:
            self._record.check()
            self._task = asyncio.ensure_future(
                self._peer._request_async(
                    "await", {"reference": self._record.id}, None, self._record.origin
                )
            )
        return _awaited_plain(self._task).__await__()

    def __repr__(self) -> str:
        return "<awaitable result from the host>"


async def _awaited_plain(task: asyncio.Future) -> Any:
    return _plain(await task)


class RemoteObject:
    """An object with methods of another process (#687 §四): every public
    name is a method, an asynchronous call of the original object
    (`await cache.get("a")`); it has no fields. One proxy per object; passed
    back to its process, it is the original there."""

    def __init__(self, peer: "Peer", record: "_Import"):
        self._peer = peer
        self._record = record

    def __getattr__(self, name: str) -> Any:
        if name.startswith("_"):
            raise AttributeError(name)
        # The method keeps the proxy alive: one taken off it keeps the
        # object, as a function reference would.
        peer, record, proxy = self._peer, self._record, self

        async def method(*args: Any) -> Any:
            assert proxy is not None
            record.check()
            return _plain(await peer._request_async("call", {"reference": record.id, "method": name}, list(args)))

        method.__name__ = name
        return method

    def __repr__(self) -> str:
        return "<remote object>"


def _method(value: Any, name: str) -> Any:
    """The method `name` of `value`, or None: looked up without running
    properties (a property is no method, whatever it returns)."""
    if name.startswith("_"):
        return None
    try:
        found = inspect.getattr_static(value, name)
    except AttributeError:
        return None
    if isinstance(found, property) or not callable(found) and not isinstance(found, (staticmethod, classmethod)):
        return None
    return getattr(value, name)


def _methods_of(value: Any) -> list:
    """The methods an object passed by reference has: its public callable
    attributes, sorted (properties are not run)."""
    return sorted(name for name in dir(value) if _method(value, name) is not None)


def no_object_method(value: Any, method: str) -> BaseException:
    """A call of `method` on `value`, an object passed by reference, that
    it has no method of (#687 §八): code `no-method`, data `{object,
    method, methods}`."""
    owner, methods = type(value).__name__, _methods_of(value)
    try:
        present = not method.startswith("_") and (inspect.getattr_static(value, method) is not None or True)
    except AttributeError:
        present = False
    if present:
        why = f"{method} is a field; objects passed by reference expose only their methods"
    else:
        why = f"methods: {', '.join(methods)}" if methods else "no methods"
    error = AttributeError(f"{owner} has no method {method} ({why})")
    error.code = "no-method"
    error.data = {"object": owner, "method": method, "methods": methods}
    return error


def _plain(value: Any) -> Any:
    """A result as a plugin sees it: the wire's undefined is None."""
    return None if value is UNDEFINED else value


@dataclasses.dataclass
class _Import:
    id: int
    kind: str
    origin: list
    grants: int = 1
    released: bool = False
    proxy: Any = None  # weakref to the proxy

    def check(self) -> None:
        if self.released:
            raise RuntimeError("this function or result from the host was released")


@dataclasses.dataclass
class _Export:
    value: Any
    kind: str
    origin: list
    business: bool
    grants: int = 0
    result: tuple | None = None
    call: str | None = None
    # The call it is the result of (`service.method`), for messages.
    described: str | None = None
    # Whose code it is (`Peer.owner` when exported): its calls run so.
    owner: Any = None


@dataclasses.dataclass(eq=False)
class _Job:
    frame: dict
    args: Any
    entry: _Export | None
    business: bool


_CLOSED = object()
# How the end of a session that failed (rather than ended) begins.
HOST_FAILED = "the connection to the host failed: "


class Peer:
    """`dispatch(target, method, args)` serves `invoke`; `settled()` runs after
    a call or property read on an exported reference returns."""

    def __init__(
        self,
        channel,
        dispatch: Callable,
        settled: Callable | None = None,
        reentrant: bool = True,
        *,
        endpoint: dict,
        require: dict | None = None,
    ):
        """`endpoint` ({"local", "expected"?, "verified"?, "declare"?}): this
        side's endpoint id, the far end's it expects, and the capabilities it
        declares beyond the session's (`declare`, such as the contract).
        `require` ({capability: why}): the session ends right after the
        handshake, with `Incompatible(why)`, when the far end does not declare
        one of them (the runtime requires `events-v2`)."""
        self._reentrant = reentrant
        self._require = dict(require or {})
        self.loop = asyncio.get_running_loop()
        self._thread = threading.get_ident()
        # A socket is a newline-framed channel.
        self._channel = SocketChannel(channel) if isinstance(channel, socket.socket) else channel
        self._write_lock = threading.Lock()
        self._dispatch = dispatch
        self._settled = settled
        # `owner()`: whose code is running now (the runtime's row, #688),
        # kept with each function or object exported; `within(owner, call)`
        # runs a call of it as that owner's.
        self.owner: Callable[[], Any] = lambda: None
        self.within: Callable[[Any, Callable], Any] = lambda _owner, call: call()
        self._inbox: collections.deque = collections.deque()
        self._cv = threading.Condition()
        self._wake_pending = False
        self._next = 0
        self._received = 0
        self._ref = 0
        self._pending: dict[str, Callable] = {}
        self._cancelled: set[str] = set()
        self._exports: dict[int, _Export] = {}
        self._identities: dict[int, int] = {}
        self._imports: dict[int, _Import] = {}
        self._waiting: list[str] = []
        self._queued: dict[_Job, None] = {}
        self._sync_path: list | None = None
        self._context: contextvars.ContextVar[list] = contextvars.ContextVar("rutis_path", default=[])
        self._tasks: dict[str, asyncio.Future] = {}
        self._signals: dict[str, Signal] = {}
        # Calls whose method is running its first step, and those of them
        # cancelled meanwhile (a cancel read during a synchronous call the
        # first step makes): the task, once it exists, is cancelled.
        self._starting: set[str] = set()
        self._cancel_early: set[str] = set()
        # (call, result) while a call's result is encoded.
        self._answering: tuple[str, Any] | None = None
        self._decoding_for: str | None = None
        # The imports a value being decoded made, released if it is refused.
        self._new_imports: list | None = None
        self._active = 0
        self._draining: list[asyncio.Future] = []
        self._handshake = False
        # Call id prefixes: `<endpoint>:`, each side's own.
        self._endpoint = endpoint
        if not _ENDPOINT_ID.match(endpoint["local"]):
            raise ValueError(f"invalid endpoint id {endpoint['local']}")
        self._local = f"{endpoint['local']}:"
        self._remote: str | None = None
        # What the far end said of itself, once it greeted.
        self.greeting: dict | None = None
        # What the far end declared, once it greeted.
        self._declared: list = []
        self.closed_error: BaseException | None = None
        self.ready: asyncio.Future = self.loop.create_future()
        self.closed: asyncio.Future = self.loop.create_future()
        self._reader = threading.Thread(target=self._read, name="rutis-reader", daemon=True)

    # ── I/O ──────────────────────────────────────────────────────

    def start(self) -> None:
        # Greet before reading (#239): a side that finds the far end's
        # greeting incompatible closes the channel, and the far end must
        # still read this side's greeting ahead of the channel end.
        hello = {
            "op": "hello",
            "version": ENDPOINT_PROTOCOL,
            "endpoint": self._endpoint["local"],
            "implementation": IMPLEMENTATION,
            "capabilities": CAPABILITIES + list(self._endpoint.get("declare", [])),
        }
        try:
            self._send(hello)
        except OSError:
            # The far end may have greeted and closed before this side
            # greeted. Its greeting is still on the channel: the reader reads
            # it, and `ready` reports it as incompatible, or else the end.
            pass
        self._reader.start()

    def supports(self, capability: str) -> bool:
        return self.greeting is not None and capability in self.greeting["capabilities"]

    def _greet(self, frame: dict) -> None:
        version = frame.get("version")
        if version != ENDPOINT_PROTOCOL:
            raise ValueError(
                f"incompatible session: {version_mismatch(version, ENDPOINT_PROTOCOL, frame.get('implementation'))}"
            )
        endpoint = frame.get("endpoint")
        if not isinstance(endpoint, str) or not _ENDPOINT_ID.match(endpoint):
            raise ValueError("incompatible session: the far end named no valid endpoint")
        for whose in ("verified", "expected"):
            expected = self._endpoint.get(whose)
            if expected is not None and expected != endpoint:
                raise ValueError(
                    f"endpoint mismatch: the far end greeted as {endpoint}, but {expected} is the {whose} endpoint"
                )
        if endpoint == self._endpoint["local"]:
            raise ValueError(f"endpoint mismatch: the far end greeted as this endpoint ({endpoint})")
        self._remote = f"{endpoint}:"
        capabilities = frame.get("capabilities")
        self.greeting = {
            "endpoint": endpoint,
            "implementation": frame.get("implementation"),
            "capabilities": capabilities if isinstance(capabilities, list) else [],
        }
        self._declared = self.greeting["capabilities"]
        self._required()

    def _required(self) -> None:
        for capability, why in self._require.items():
            if capability not in self._declared:
                raise Incompatible(why)

    def _read(self) -> None:
        try:
            while (message := self._channel.recv()) is not None:
                # Strict UTF-8, and no BOM: json.loads would take bytes that
                # start with one, where Rust and Node refuse the frame.
                self._deliver(json.loads(message.decode("utf-8")))
        except Exception as error:  # noqa: BLE001 - any failure ends the session
            # Text that is not JSON ends the channel too, not only the session.
            try:
                self._channel.close(str(error))
            except OSError:
                pass
            self._deliver((_CLOSED, f"{HOST_FAILED}{error}"))
            return
        self._deliver((_CLOSED, "the host disconnected"))

    def _deliver(self, item: Any) -> None:
        with self._cv:
            self._inbox.append(item)
            self._cv.notify_all()
            wake = not self._wake_pending
            self._wake_pending = True
        if wake:
            try:
                self.loop.call_soon_threadsafe(self._drain)
            except RuntimeError:
                pass  # the loop is gone

    def _drain(self) -> None:
        with self._cv:
            self._wake_pending = False
        while True:
            with self._cv:
                if not self._inbox:
                    return
                item = self._inbox.popleft()
            if isinstance(item, tuple) and item and item[0] is _CLOSED:
                failed = item[1].startswith(HOST_FAILED)
                self.close(host_gone(item[1], "failed" if failed else "disconnected"))
            else:
                self.receive(item)

    def _send(self, frame: dict) -> None:
        if self.closed_error is not None:
            raise self.closed_error
        data = dumps(frame)
        with self._write_lock:
            self._channel.send(data)

    def close(self, error: BaseException | None = None) -> None:
        if self.closed_error is not None:
            return
        self.closed_error = error or host_gone()
        if not self.ready.done():
            self.ready.set_exception(self.closed_error)
            self.ready.exception()  # retrieved: nothing may wait for it
        for finish in list(self._pending.values()):
            finish((False, self.closed_error))
        self._pending.clear()
        self._exports.clear()
        self._imports.clear()
        self._queued.clear()
        with self._cv:
            self._cv.notify_all()
        for waiter in self._draining:
            if not waiter.done():
                waiter.set_result(None)
        if not self.closed.done():
            self.closed.set_result(None)

    def _fault(self, error: BaseException) -> None:
        self.close(error)
        try:
            self._channel.close(str(error))
        except OSError:
            pass

    # ── Call chains ──────────────────────────────────────────────

    def _path(self) -> list:
        if self._sync_path is not None:
            return self._sync_path
        return self._context.get()

    def _allocate(self) -> str:
        if self.closed_error is not None:
            raise self.closed_error
        if not self._handshake:
            raise RuntimeError("protocol handshake incomplete")
        self._next += 1
        if self._next > MAX_SAFE:
            raise RuntimeError("call identifiers exhausted")
        return f"{self._local}{self._next}"

    # ── Values ───────────────────────────────────────────────────

    def _encode(self, value: Any, grants: list, business: bool, depth: int = 0) -> dict:
        """The wire form of `value`; `depth` counts the levels around it (an
        argument list starts at -1: it is not a level)."""
        if value is UNDEFINED:
            return {"type": "undefined"}
        if isinstance(value, (RemoteFunction, RemoteFuture, RemoteObject)):
            record = value._record
            record.check()
            return {
                "type": "reference",
                "value": {"id": record.id, "kind": record.kind, "home": True, "origin": record.origin},
            }
        if asyncio.iscoroutine(value):
            value = self._task(value)
        if isinstance(value, Plugin):
            # A plugin definition (`define_plugin`) crosses as itself: a
            # `plugin` reference, sent only to a far end that declared
            # `plugins` (docs/design-plugin-values-2026-10-25.md §5).
            if "plugins" not in self._declared:
                raise ValueError(
                    "cannot send a plugin definition: the far end does not accept plugin definitions "
                    "(it did not declare the capability plugins)"
                )
        if (
            (callable(value) and not isinstance(value, type))
            or asyncio.isfuture(value)
            or _is_live(value)
            or isinstance(value, Plugin)
        ):
            key = id(value)
            ref = self._identities.get(key)
            entry = self._exports.get(ref) if ref is not None else None
            if entry is None or entry.value is not value:
                self._ref += 1
                ref = self._ref
                kind = (
                    "future"
                    if asyncio.isfuture(value)
                    else "plugin"
                    if isinstance(value, Plugin)
                    else "function"
                    if callable(value)
                    else "object"
                )
                if kind == "object" and not self.supports("objects"):
                    # Only a far end that declared `objects` can call its
                    # methods (#675).
                    far = self.greeting["endpoint"]
                    raise ValueError(
                        f"an object with methods ({type(value).__name__}) cannot cross to "
                        f"{far}; pass plain data or a function instead, or upgrade {far} to rutis 0.18"
                    )
                entry = _Export(value, kind, list(self._path()), business, owner=self.owner())
                # The result of a call: awaiting it is awaiting that call.
                if kind == "future" and self._answering is not None and self._answering[1] is value:
                    entry.call = self._answering[0]
                    entry.described = self._answering[2]
                self._identities[key] = ref
                self._exports[ref] = entry
                if kind == "future":
                    if business:
                        self._active += 1
                    value.add_done_callback(lambda done, entry=entry: self._future_done(entry, done))
            entry.grants += 1
            grants.append(ref)
            return {
                "type": "reference",
                "value": {"id": ref, "kind": entry.kind, "home": False, "origin": entry.origin},
            }
        tag = _values.tag_of(value)
        if tag is not None:
            return self._encode_tagged(tag, value, grants, business, depth)
        if isinstance(value, (list, tuple, dict, BaseException)) and depth >= MAX_DEPTH:
            raise _too_deep()
        if isinstance(value, (list, tuple)):
            # An argument list keeps an undefined argument as one.
            if _beyond_data(value) or (depth < 0 and any(item is UNDEFINED for item in value)):
                return {
                    "type": "list",
                    # In data undefined is null (design V10); an argument
                    # list keeps it, as an omitted argument.
                    "value": [
                        _NULL if item is UNDEFINED and depth >= 0 else self._encode(item, grants, business, depth + 1)
                        for item in value
                    ],
                }
            return {"type": "data", "value": _data(value, depth)}
        if isinstance(value, BaseException):
            return {"type": "data", "value": {"name": type(value).__name__, "message": str(value)}}
        if isinstance(value, dict) and _beyond_data(value):
            return {
                "type": "record",
                "value": {
                    key: _NULL if item is UNDEFINED else self._encode(item, grants, business, depth + 1)
                    for key, item in value.items()
                },
            }
        if dataclasses.is_dataclass(value) and not isinstance(value, type) and _beyond_data(value):
            # A dataclass crosses as a dict of its fields (asdict copies
            # them): tagged values in it as such, but no references.
            try:
                fields = dataclasses.asdict(value)
            except RecursionError:
                raise _too_deep() from None
            if _holds_reference(fields):
                raise TypeError(
                    f"{type(value).__name__} cannot cross the boundary: a dataclass crosses as data, "
                    "without functions or objects with behaviour"
                )
            return self._encode(fields, grants, business, depth)
        return {"type": "data", "value": _data(value, depth)}

    def _encode_tagged(self, tag: str, value: Any, grants: list, business: bool, depth: int) -> dict:
        """A tagged value (#308), sent only to a far end that declared
        `value-<tag>`: to any other, the call carrying it fails."""
        capability = f"value-{tag}"
        if capability not in self._declared:
            raise ValueError(
                f"cannot send {_values.describe(value)}: the far end does not take {tag} values "
                f"(it did not declare the capability {capability})"
            )
        if tag not in ("map", "set"):
            return {"type": tag, "value": _values.write(tag, value)}
        if depth >= MAX_DEPTH:
            raise _too_deep()
        if tag == "set":
            elements = list(value)
            _values.check_keys(elements, "set element")
            return {"type": "set", "value": [self._encode(item, grants, business, depth + 1) for item in elements]}
        entries = list(value.items())
        _values.check_keys((key for key, _item in entries), "map key")
        return {
            "type": "map",
            "value": [
                [
                    self._encode(key, grants, business, depth + 1),
                    # Undefined is null in data (design V10).
                    _NULL
                    if item is UNDEFINED
                    else self._encode(item, grants, business, depth + 1),
                ]
                for key, item in entries
            ],
        }

    def _future_done(self, entry: _Export, done: asyncio.Future) -> None:
        # Recorded early when an await needed it first (see _execute).
        if entry.result is None:
            entry.result = _outcome(done)
        if entry.business:
            self._finish()

    def _rollback(self, grants: list) -> None:
        for ref in grants:
            entry = self._exports.get(ref)
            if entry is None:
                continue
            entry.grants -= 1
            if entry.grants == 0:
                self._forget(ref)

    def _forget(self, ref: int) -> None:
        entry = self._exports.pop(ref, None)
        if entry is not None and self._identities.get(id(entry.value)) == ref:
            del self._identities[id(entry.value)]

    def _decode(self, wire: Any) -> Any:
        kind = wire.get("type") if isinstance(wire, dict) else None
        if kind == "undefined":
            return UNDEFINED
        if kind == "data":
            return wire["value"]
        # Undefined inside a value is None at any depth, also from an older
        # far end that sends it there (#687 §七).
        if kind == "list":
            return [_plain(item) for item in self._decode_each(wire["value"])]
        if kind == "record":
            if not isinstance(wire["value"], dict):
                raise ValueError("invalid record")
            fields = wire["value"]
            return dict(zip(fields, [_plain(item) for item in self._decode_each(fields.values())]))
        if kind == "signal":
            if self._decoding_for is None:
                raise ValueError("a signal is only valid as a call argument")
            return self._signals.setdefault(self._decoding_for, Signal())
        if kind == "reference":
            return self._decode_reference(wire["value"])
        if kind in _values.TAGS:
            return self._decode_tagged(kind, wire.get("value"))
        raise ValueError("invalid wire value")

    def _decode_each(self, wires: Any) -> list:
        """Decode every one of `wires`, even past a refused one (so each
        reference in them is imported, and released if the call fails);
        then raise the first refusal. A malformed value raises at once: it
        ends the session."""
        decoded, refused = [], None
        for wire in wires:
            try:
                decoded.append(self._decode(wire))
            except _values.Refused as error:
                refused = refused or error
        if refused is not None:
            raise refused
        return decoded

    def _decode_tagged(self, tag: str, value: Any) -> Any:
        """A tagged value (#308). The wrong JSON shape is a malformed frame
        (ValueError, the session ends); text not in the tag's format, or
        map keys and set elements against the rules, refuse the value
        (`_values.Refused`: only its call fails)."""
        if tag == "map":
            if not isinstance(value, list) or not all(isinstance(entry, list) and len(entry) == 2 for entry in value):
                raise ValueError("invalid map: expected an array of [key, value] pairs")
            parts = self._decode_each([part for entry in value for part in entry])
            # Undefined is null in data (design V10).
            items = [None if item is UNDEFINED else item for item in parts[1::2]]
            return _values.received_map(parts[0::2], items)
        if tag == "set":
            if not isinstance(value, list):
                raise ValueError("invalid set: expected an array")
            return _values.received_set(self._decode_each(value))
        if not isinstance(value, str):
            raise ValueError(f"invalid {tag}: expected a string")
        return _values.read(tag, value)

    def _decode_checked(self, wire: Any, arguments: bool = False) -> tuple:
        """(True, the value), or (False, ValueError) when it holds a value
        this side refuses: only the call it came in fails, and the
        references it carried are released at once."""
        imported: list = []
        outer, self._new_imports = self._new_imports, imported
        try:
            # Each argument alone: a whole argument keeps undefined.
            if arguments and isinstance(wire, dict) and wire.get("type") == "list" and isinstance(wire.get("value"), list):
                return True, self._decode_each(wire["value"])
            return True, self._decode(wire)
        except _values.Refused as refused:
            for record in imported:
                self._release(record)
            return False, ValueError(str(refused))
        finally:
            self._new_imports = outer

    def _decode_reference(self, value: dict) -> Any:
        ref, home, kind, origin = value.get("id"), value.get("home"), value.get("kind"), value.get("origin")
        if (
            not isinstance(ref, int)
            or not 0 < ref <= MAX_SAFE
            or kind not in ("function", "future", "object", "plugin")
            or not isinstance(home, bool)
            or not isinstance(origin, list)
            or not all(isinstance(item, str) and _ENDPOINT_ORIGIN.match(item) for item in origin)
        ):
            raise ValueError("invalid reference")
        if home:
            return self._export(ref, kind).value
        # Plugin definitions travel back home only: this side's own.
        if kind == "plugin":
            raise ValueError(
                "received a plugin definition made in another process; only one made in this process "
                "can come back to it"
            )
        record = self._imports.get(ref)
        proxy = record.proxy() if record is not None else None
        if proxy is not None:
            if record.kind != kind or record.origin != origin:
                raise ValueError("invalid repeated grant")
            record.grants += 1
            return proxy
        record = _Import(ref, kind, origin)
        if self._new_imports is not None:
            self._new_imports.append(record)
        proxy = (
            RemoteFunction(self, record)
            if kind == "function"
            else RemoteObject(self, record)
            if kind == "object"
            else RemoteFuture(self, record)
        )
        record.proxy = weakref.ref(proxy)
        self._imports[ref] = record
        weakref.finalize(proxy, self._release_soon, record)
        return proxy

    def _release_soon(self, record: _Import) -> None:
        # Finalizers run on whatever thread collected the proxy.
        try:
            self.loop.call_soon_threadsafe(self._release, record)
        except RuntimeError:
            pass

    def _release(self, record: _Import) -> None:
        if record.released:
            return
        record.released = True
        if self._imports.get(record.id) is record:
            del self._imports[record.id]
        if self.closed_error is None:
            try:
                self._send({"op": "release", "reference": record.id, "count": record.grants})
            except Exception as error:  # noqa: BLE001
                self._fault(error)

    def release(self, proxy: Any) -> None:
        """Release an imported reference now instead of at collection."""
        self._release(proxy._record)

    def references(self) -> dict:
        """Reference table sizes: exported (granted, not yet released by the
        far end) and imported (not yet released). Diagnostics for tests
        (risk P4)."""
        return {"exports": len(self._exports), "imports": len(self._imports)}

    def _export(self, ref: int, kind: str | None = None) -> _Export:
        entry = self._exports.get(ref)
        if entry is None or (kind is not None and entry.kind != kind):
            raise ValueError("unknown, released or mismatched reference")
        return entry

    # ── Outgoing calls ───────────────────────────────────────────

    def _request(
        self, op: str, fields: dict, args: Any, finish: Callable, origin: list | None = None, sync: bool = False
    ) -> str:
        call = self._allocate()
        grants: list = []
        try:
            path = list(dict.fromkeys([*self._path(), *(origin or [])]))
            frame = {"op": op, "id": call, "path": path, **fields}
            # This thread waits for the reply: a host that checks for wait
            # cycles needs to know (#228); older hosts would refuse the field.
            if sync and "sync-wait" in self._declared:
                frame["sync"] = True
            described = _described(op, fields)
            if op != "await":
                frame["args"] = self._encode_args(args, grants, described)
            data = dumps(frame)
            # A message over the limit would end the channel: only this call
            # fails, before it is sent (#675).
            limit = self._max_message()
            if len(data) > limit:
                reason = _over_limit(len(data), limit)
                index = _largest(frame.get("args"))
                raise _cannot_send(
                    ValueError(reason), described, "argument", index, limit, None if index is not None else "the arguments"
                )
            self._pending[call] = finish
            if self.closed_error is not None:
                raise self.closed_error
            try:
                with self._write_lock:
                    self._channel.send(data)
            except OSError as error:
                # The channel failed: the session is over, and later calls
                # fail with that, not on a closed descriptor each.
                self._pending.pop(call, None)
                self._fault(error)
                raise
        except BaseException:
            self._pending.pop(call, None)
            self._rollback(grants)
            raise
        return call

    def _max_message(self) -> int:
        """The channel's limit on one message (16 MiB unless it says)."""
        limit = getattr(self._channel, "max_message", None)
        return limit if isinstance(limit, int) and not isinstance(limit, bool) else MAX_MESSAGE

    def _encode_args(self, args: Any, grants: list, call: str) -> dict:
        """The wire form of a call's arguments (a list; the list is not a
        level), as `_encode` makes it: an error says which argument of
        `call` it is (#675)."""
        if not isinstance(args, (list, tuple)):
            return self._encode(args, grants, True, -1)
        # An argument list keeps an undefined argument as one.
        listed = _beyond_data(args) or any(item is UNDEFINED for item in args)
        encoded = []
        for index, item in enumerate(args):
            try:
                encoded.append(self._encode(item, grants, True, 0) if listed else _data(item, 0))
            except (ValueError, TypeError, RuntimeError) as error:
                raise _cannot_send(error, call, "argument", index) from error
        return {"type": "list" if listed else "data", "value": encoded}

    def _call_sync(self, op: str, fields: dict, args: Any) -> Any:
        if threading.get_ident() != self._thread:
            # A foreign thread: let the loop make the call, and block only
            # this thread.
            future = asyncio.run_coroutine_threadsafe(self._request_async(op, fields, args), self.loop)
            return future.result()
        result: list = []
        call = self._request(op, fields, args, result.append, sync=True)
        self._waiting.append(call)
        try:
            for job in list(self._queued):
                self._run(job)
            while not result:
                with self._cv:
                    while not self._inbox and not result and self.closed_error is None:
                        self._cv.wait()
                if self.closed_error is not None and not result and not self._inbox:
                    raise self.closed_error
                self._drain()
        finally:
            self._waiting.pop()
            if not self._waiting:
                for job in list(self._queued):
                    self.loop.call_soon(self._run, job)
        ok, value = result[0]
        if not ok:
            raise value
        return value

    async def _request_async(self, op: str, fields: dict, args: Any, origin: list | None = None) -> Any:
        future = self.loop.create_future()

        def finish(result: tuple) -> None:
            if future.done():
                return
            ok, value = result
            if ok:
                future.set_result(value)
            else:
                future.set_exception(value)

        call = self._request(op, fields, args, finish, origin)
        try:
            return await future
        except asyncio.CancelledError:
            # Tell the other side; its late reply is dropped.
            if self._pending.pop(call, None) is not None:
                self._cancelled.add(call)
                try:
                    self._send({"op": "cancel", "id": call})
                except Exception:  # noqa: BLE001
                    pass
            raise

    def call(self, target: str, method: str, args: list) -> Any:
        return self._call_sync("invoke", {"target": target, "method": method}, args)

    async def call_async(self, target: str, method: str, args: list) -> Any:
        result = await self._request_async("invoke", {"target": target, "method": method}, args)
        if isinstance(result, RemoteFuture):
            result = await result
        return result

    def request_answered(self, target: str, method: str, args: list, answered: Callable[[bool, Any], None]) -> None:
        """Send a call nobody waits for; `answered(ok, value)` is told its
        reply as the reply's frame is taken, before the next frame (or the
        session's end), not on a later turn of the loop (events protocol
        §5.5). From another thread it is sent from the session's loop."""

        def finish(result: tuple) -> None:
            answered(*result)

        def send() -> None:
            try:
                self._request("invoke", {"target": target, "method": method}, args, finish)
            except BaseException as error:  # noqa: BLE001 - told as the answer
                answered(False, error)

        if threading.get_ident() != self._thread:
            self.loop.call_soon_threadsafe(send)
            return
        self._request("invoke", {"target": target, "method": method}, args, finish)

    def request(self, target: str, method: str, args: list):
        """Send a call now, before this returns, so it goes out ahead of
        anything sent later; the future of its reply. From another thread
        it is sent from the session's loop, and the future is a
        `concurrent.futures.Future`."""
        if threading.get_ident() != self._thread:
            return asyncio.run_coroutine_threadsafe(self.call_async(target, method, args), self.loop)
        future = self.loop.create_future()

        def finish(result: tuple) -> None:
            if future.done():
                return
            ok, value = result
            if ok:
                future.set_result(value)
            else:
                future.set_exception(value)

        self._request("invoke", {"target": target, "method": method}, args, finish)
        return future

    def notify(self, target: str, method: str, args: list) -> None:
        """Fire and forget: a call whose result nobody awaits. It is sent
        before this returns, so it goes out ahead of anything sent later,
        such as the reply of the call that caused it."""
        if self.closed_error is not None:
            return
        if threading.get_ident() != self._thread:
            self.loop.call_soon_threadsafe(self.notify, target, method, args)
            return
        try:
            self._request("invoke", {"target": target, "method": method}, args, lambda _result: None)
        except Exception:  # noqa: BLE001 - nobody awaits a notification
            pass

    async def drain(self) -> None:
        if self._active:
            waiter = self.loop.create_future()
            self._draining.append(waiter)
            await waiter

    def _finish(self) -> None:
        self._active -= 1
        if self._active == 0:
            for waiter in self._draining:
                if not waiter.done():
                    waiter.set_result(None)
            self._draining.clear()

    # ── Incoming frames ──────────────────────────────────────────

    def receive(self, frame: dict) -> None:
        if self.closed_error is not None:
            return
        try:
            self._receive(frame)
        except Exception as error:  # noqa: BLE001 - a bad frame ends the session
            self._fault(error)

    def _receive(self, frame: dict) -> None:
        op = frame.get("op")
        if op == "hello":
            if self._handshake:
                raise ValueError("duplicate protocol handshake")
            self._greet(frame)
            self._handshake = True
            if not self.ready.done():
                self.ready.set_result(None)
            return
        if not self._handshake:
            raise ValueError("request before protocol handshake")
        if op in ("return", "throw"):
            error = frame.get("error")
            if op == "throw" and not (
                isinstance(error, dict) and isinstance(error.get("name"), str) and isinstance(error.get("message"), str)
            ):
                raise ValueError("invalid error")
            # A value this side refuses fails the call waiting for it.
            ok, value = self._decode_checked(frame["value"]) if op == "return" else (False, decode_error(frame["error"]))
            call = frame.get("id")
            finish = self._pending.pop(call, None)
            if finish is None:
                if call in self._cancelled:
                    self._cancelled.discard(call)
                    return
                raise ValueError("response for unknown call")
            finish((ok, value))
            return
        if op == "cancel":
            call = frame.get("id")
            if not isinstance(call, str):
                raise ValueError("invalid cancel")
            signal = self._signals.get(call)
            if signal is not None:
                signal._cancel()
            task = self._tasks.get(call)
            if task is not None:
                task.cancel()
            elif call in self._starting:
                self._cancel_early.add(call)
            return
        if op == "release":
            entry = self._export(frame.get("reference"))
            count = frame.get("count")
            if not isinstance(count, int) or not 0 < count <= entry.grants:
                raise ValueError("invalid reference release count")
            entry.grants -= count
            if entry.grants == 0:
                self._forget(frame["reference"])
            return
        call, path = frame.get("id"), frame.get("path")
        if (
            op not in ("invoke", "call", "get", "await")
            or not isinstance(call, str)
            or not call.startswith(self._remote)
            or not _SEQUENCE.match(call[len(self._remote):])
            or not isinstance(path, list)
            or not all(isinstance(item, str) for item in path)
            or call in path
        ):
            raise ValueError("invalid invocation")
        sequence = int(call[len(self._remote):])
        if sequence <= self._received or sequence > MAX_SAFE:
            raise ValueError("invalid or repeated invocation identity")
        self._received = sequence
        if op == "invoke" and not (isinstance(frame.get("target"), str) and isinstance(frame.get("method"), str)):
            raise ValueError("invalid target or method")
        if op == "call" and frame.get("method") is not None and not isinstance(frame["method"], str):
            raise ValueError("invalid method")
        if op == "get" and not isinstance(frame.get("property"), str):
            raise ValueError("invalid property")
        args = None
        if op in ("invoke", "call"):
            self._decoding_for = call
            try:
                ok, args = self._decode_checked(frame.get("args"), arguments=True)
            finally:
                self._decoding_for = None
            if not ok:
                # Arguments this side refuses: only this call fails (#308).
                self._signals.pop(call, None)
                self._send({"op": "throw", "id": call, "error": encode_error(args)})
                return
        if op == "await":
            kind = "future"
        elif op == "get" or frame.get("method") is not None:
            kind = "object"
        else:
            kind = "function"
        entry = None if op == "invoke" else self._export(frame.get("reference"), kind)
        business = entry.business if entry is not None else frame.get("target") != ""
        if business:
            self._active += 1
        job = _Job(frame, args, entry, business)
        self._queued[job] = None
        if self._waiting:
            self._run(job)
        else:
            self.loop.call_soon(self._run, job)

    def _related(self, job: _Job) -> bool:
        return any(call in job.frame["path"] for call in self._waiting)

    def _run(self, job: _Job) -> None:
        if job not in self._queued:
            return
        if self._waiting and not self._reentrant and not self._related(job):
            return
        del self._queued[job]
        self._execute(job)

    def _execute(self, job: _Job) -> None:
        frame, entry, business = job.frame, job.entry, job.business
        if self.closed_error is not None:
            if business:
                self._finish()
            return
        call = frame["id"]
        path = [*frame["path"], call]
        previous = self._sync_path
        self._sync_path = path
        token = self._context.set(path)
        try:
            if frame["op"] == "await":
                if entry.result is None and entry.value.done():
                    # Done, but its done-callback has not run yet: asyncio
                    # schedules it, and a synchronous call may hold the loop.
                    entry.result = _outcome(entry.value)
                if entry.result is not None:
                    self._respond(call, entry.result, business, entry.described)
                elif self._waiting and self._related(job):
                    # Said with what the plugin wrote, not the call ids.
                    awaited = entry.described or "awaiting a result"
                    cycle = SyncWaitCycle(
                        f"{awaited} would wait on itself: its result is awaited while the synchronous "
                        "call waiting for it holds this Python process (synchronous calls)"
                    )
                    self._respond(call, (False, cycle), business)
                else:
                    if entry.call is not None:
                        self._tasks[call] = self._tasks.get(entry.call) or entry.value
                        # Cancelling this await is cancelling that call.
                        signal = self._signals.get(entry.call)
                        if signal is not None:
                            self._signals[call] = signal
                    entry.value.add_done_callback(
                        lambda _done: self._respond(call, entry.result, business, entry.described)
                    )
                return
            value = None
            self._starting.add(call)
            try:
                try:
                    if frame["op"] == "get":
                        value = getattr(entry.value, frame["property"])
                    elif frame["op"] == "call" and frame.get("method") is not None:
                        name = frame["method"]
                        method = _method(entry.value, name)
                        if method is None:
                            raise no_object_method(entry.value, name)
                        value = self.within(entry.owner, lambda: _apply(method, job.args))
                    elif frame["op"] == "call":
                        value = self.within(entry.owner, lambda: _apply(entry.value, job.args))
                    else:
                        value = self._dispatch(frame["target"], frame["method"], job.args)
                    if asyncio.iscoroutine(value):
                        value = self._task(value, call)
                finally:
                    if frame["op"] != "invoke" and self._settled is not None:
                        if asyncio.isfuture(value):
                            value.add_done_callback(lambda _done: self._settled())
                        else:
                            self._settled()
                self._respond(call, (True, value), business, _described(frame["op"], frame))
            except BaseException as error:  # noqa: BLE001 - every failure is the caller's
                self._respond(call, (False, error), business, _described(frame["op"], frame))
        finally:
            self._starting.discard(call)
            self._cancel_early.discard(call)
            self._context.reset(token)
            self._sync_path = previous

    def _task(self, coroutine: Any, call: str | None = None) -> asyncio.Future:
        """A task for a returned coroutine, started at once like a JS async
        function, so a coroutine that never suspends is already done. 3.10
        and 3.11 have no native eager start: `_eager` does the same."""
        context = contextvars.copy_context()
        if sys.version_info >= (3, 12):
            task = asyncio.Task(coroutine, loop=self.loop, context=context, eager_start=True)
        else:
            task = _eager.start(self.loop, coroutine, context)
        if call is not None and not task.done():
            self._tasks[call] = task
            task.add_done_callback(lambda _done: self._tasks.pop(call, None))
            if call in self._cancel_early:
                task.cancel()
        return task

    def _respond(self, call: str, result: tuple, business: bool, described: str | None = None) -> None:
        """Answer the far end's call `call` (a call of `described`, when it
        is known, for messages). A result that cannot be sent (a value the
        far end does not take, a message over the channel's limit) is
        answered with an error saying why instead (#675)."""
        # A signal lives until the call's result settles: a returned future
        # keeps it, so cancelling an await of it still reaches it.
        ok_value = result[1] if result[0] else None
        if asyncio.isfuture(ok_value) and not ok_value.done():
            ok_value.add_done_callback(lambda _done: self._signals.pop(call, None))
        else:
            self._signals.pop(call, None)
        grants: list = []
        try:
            if self.closed_error is not None:
                return
            ok, value = result
            self._answering = (call, value, described)
            try:
                frame = (
                    {"op": "return", "id": call, "value": self._encode(value, grants, business)}
                    if ok
                    else {"op": "throw", "id": call, "error": encode_error(value)}
                )
                data = dumps(frame)
            except Exception as error:  # noqa: BLE001
                self._rollback(grants)
                grants = []
                if ok and isinstance(error, (ValueError, TypeError, RuntimeError)):
                    error = _cannot_send(error, described, "result")
                data = dumps({"op": "throw", "id": call, "error": encode_error(error)})
            finally:
                self._answering = None
            limit = self._max_message()
            if len(data) > limit:
                # Over the limit, the channel would end: the far end gets
                # why instead (#675).
                self._rollback(grants)
                name = value.name if isinstance(value, RemoteError) else type(value).__name__
                reason = _cannot_send(
                    ValueError(_over_limit(len(data), limit)),
                    described,
                    "result" if ok else "error",
                    limit=limit,
                    what=None if ok else f"the error {name}",
                )
                data = dumps({"op": "throw", "id": call, "error": encode_error(reason)})
            with self._write_lock:
                self._channel.send(data)
        except Exception as error:  # noqa: BLE001
            self._fault(error)
        finally:
            if business:
                self._finish()


def _outcome(done: asyncio.Future) -> tuple:
    if done.cancelled():
        return (False, RemoteError("CancelledError", "the call was cancelled"))
    if done.exception() is not None:
        return (False, done.exception())
    return (True, done.result())


def _apply(function: Callable, args: Any) -> Any:
    """Call with the wire arguments: a trailing `undefined` is an omitted
    argument (the default applies), an inner one is None."""
    args = list(args) if isinstance(args, list) else [] if args is None or args is UNDEFINED else [args]
    while args and args[-1] is UNDEFINED:
        args.pop()
    return function(*[None if arg is UNDEFINED else arg for arg in args])

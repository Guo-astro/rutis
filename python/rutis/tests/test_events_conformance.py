"""The testing tool against the unified events expectations (#464 step 7):
every case of crates/rutis-bridge/src/runtime/testing/events.expected.json
without `tool: false`, every participant at X (the language `python` for
`only` / `except`), with the plugin tests/fixtures/conformance_events.py.
The real runtimes are checked against the same file by
crates/rutis-loader/tests/events_conformance.rs.

The rows of a case share one `rutis.testing.Bus` (the file's `scopes`):
listener rows of tests/fixtures/conformance_events.py, emitter rows of
conformance_events_e.py / conformance_events_f.py (each declaring only its
own `emitter-<label>`), each loaded with `rutis.testing.load(..., bus=, scope=, row=<label>)`; the
host service `events-log` is EventsLog below. `viaHost` and `reported` are
about the host and real processes and do not apply here.
"""

import asyncio
import json
import re
import sys
import unittest
from collections import Counter
from contextlib import AsyncExitStack
from pathlib import Path

import background
import test_conformance as weather  # `same`: tagged-value equality

from rutis import _values
from rutis.peer import AggregateError, RemoteError
from rutis.testing import Bus, load

sys.path.insert(0, str(Path(__file__).parent / "fixtures"))
# The fixture directory is the runtime's project: listener rows load
# conformance_events, emitter rows conformance_events_<label>.
import conformance_events  # noqa: E402
import conformance_events_e  # noqa: E402
import conformance_events_f  # noqa: E402

EMITTERS = {"e": conformance_events_e, "f": conformance_events_f}

EXPECTED = json.loads(
    (Path(__file__).parents[3] / "crates/rutis-bridge/src/runtime/testing/events.expected.json").read_text("utf-8")
)
LANGUAGE = "python"
# `settles` and the wait for logs to be complete (§13.2).
SETTLE = 5.0
# Any other step: long enough never to cut a passing one short.
LIMIT = 30.0


class EventsLog:
    """The host service `events-log`: every row logs here."""

    def __init__(self):
        self.entries = []
        self.seen = []

    def add(self, entry):
        self.entries.append(entry)

    def filter(self, description):
        self.entries.append(f"filter:{description['row']}")
        self.seen.append(description)

    async def wait(self, entry):
        while entry not in self.entries:
            await asyncio.sleep(0.005)


def decode(value):
    """A value as the expectations write it ({"$<tag>": text},
    {"$callback": "upper"}) as Python has it."""
    if isinstance(value, list):
        return [decode(item) for item in value]
    if not isinstance(value, dict):
        return value
    if len(value) == 1 and next(iter(value)).startswith("$"):
        ((tag, text),) = value.items()
        tag = tag[1:]
        if tag == "callback":
            if text != "upper":
                raise ValueError(f"no callback {text!r}")
            return lambda text: text.upper()
        if tag == "map":
            return _values.received_map([decode(key) for key, _ in text], [decode(item) for _, item in text])
        if tag == "set":
            return _values.received_set([decode(item) for item in text])
        return _values.read(tag, text)
    return {key: decode(item) for key, item in value.items()}


def name_of(error):
    return error.name if isinstance(error, (RemoteError, AggregateError)) else type(error).__name__


def message_of(error):
    return error.message if isinstance(error, (RemoteError, AggregateError)) else str(error)


def contains(log, wanted):
    """Whether `wanted` is in `log` in this order, others between."""
    rest = iter(log)
    return all(any(entry == item for entry in rest) for item in wanted)


def excluded(case):
    """Why Python does not take part in `case`, or None."""
    for rule in case.get("only", []):
        if any(key != "why" and LANGUAGE not in languages for key, languages in rule.items()):
            return rule["why"]
    for rule in case.get("except", []):
        if any(key != "why" and LANGUAGE in languages for key, languages in rule.items()):
            return rule["why"]
    return None


def emits_of(steps):
    for step in steps:
        if "emit" in step:
            yield step["emit"]
        yield from emits_of(step.get("concurrent", []))


class Run:
    """One case: its rows on one bus, its steps in order."""

    def __init__(self, test, case):
        self.test = test
        self.case = case
        self.log = EventsLog()
        self.bus = Bus(scopes=EXPECTED["scopes"])
        # label -> (loaded row, its exit stack, scope, config)
        self.rows = {}
        self.background = None
        # A listener's default kind (§13.2).
        sync = all(spec["mode"] in ("bail", "waterfall-sync") for spec in emits_of(case["steps"]))
        self.kind = "sync" if sync and any(True for _ in emits_of(case["steps"])) else "async"

    async def run(self):
        try:
            for listener in self.case["listeners"]:
                await self.load(listener)
            for i, step in enumerate(self.case["steps"]):
                with self.test.subTest(step=i):
                    await self.step(f"{self.case['case']} step {i}", step)
        finally:
            if self.background is not None and not self.background.done():
                self.background.cancel()
                await asyncio.gather(self.background, return_exceptions=True)
            for label in list(self.rows)[::-1]:
                await self.unload(label)

    async def add(self, label, config, scope):
        module = EMITTERS[label] if config.get("emitter") else conformance_events
        stack = AsyncExitStack()
        try:
            row = await stack.enter_async_context(
                load(module, config=config, services={"events-log": self.log}, bus=self.bus, scope=scope, row=label)
            )
        except BaseException:
            await stack.aclose()
            raise
        self.rows[label] = (row, stack, scope, config)
        return row

    async def unload(self, label):
        _row, stack, _scope, _config = self.rows.pop(label)
        await stack.aclose()

    async def load(self, listener):
        config = {
            "label": listener["label"],
            "name": listener.get("name", self.case["name"]),
            "kind": listener.get("kind", self.kind),
            "log": listener.get("log", "label"),
            "does": listener.get("does", {"returns": None}),
        }
        for key in ("options", "provides", "fails"):
            if key in listener:
                config[key] = listener[key]
        scope = listener.get("scope", "root")
        return await self.add(listener["label"], config, None if scope == "root" else scope)

    async def emitter(self, label, scope):
        """The service of emitter row `label` at `scope`, loaded (again)
        when it is not there yet."""
        held = self.rows.get(label)
        if held is not None and held[2] != scope:
            await self.unload(label)
            held = None
        row = held[0] if held is not None else await self.add(label, {"label": label, "emitter": True}, scope)
        return row.service(f"emitter-{label}")

    async def dispatch(self, spec):
        scope = spec.get("scope", "root")
        service = await self.emitter(spec.get("row", "e"), None if scope == "root" else scope)
        sent = {
            "mode": spec["mode"],
            "name": spec.get("name", self.case["name"]),
            "args": [decode(arg) for arg in spec.get("args", [])],
        }
        for key in ("filter", "terminal", "cancel", "via"):
            if key in spec:
                sent[key] = spec[key]
        if spec["mode"] == "bail":
            return service.dispatch_sync(sent)
        return await service.dispatch(sent)

    async def settle(self, what, awaitable, expect):
        """(True, value) or (False, error): how `awaitable` ended."""
        limit = SETTLE if expect.get("settles") else LIMIT
        try:
            return True, await asyncio.wait_for(awaitable, limit)
        except asyncio.TimeoutError:
            self.test.fail(f"{what}: did not settle within {limit:g} s")
        except (Exception, asyncio.CancelledError) as error:  # noqa: BLE001 - what it raised is the outcome
            return False, error

    async def step(self, what, step):
        expect = step.get("expect", {})
        outcome = None
        if "emit" in step:
            if step.get("background"):
                self.background = asyncio.ensure_future(self.dispatch(step["emit"]))
                return
            outcome = await self.settle(what, self.dispatch(step["emit"]), expect)
        elif "load" in step:
            outcome = await self.settle(what, self.load(step["load"]), expect)
        elif "unload" in step:
            if "when" in step:
                await self.settle(f"{what}: waiting for {step['when']}", self.log.wait(step["when"]), {"settles": True})
            outcome = await self.settle(what, self.unload(step["unload"]), expect)
            if self.background is not None:
                outcome = await self.settle(f"{what} (the background dispatch)", self.background, expect)
                self.background = None
        elif "update" in step:
            # The row's config changes: it registers again, last.
            label = step["update"]
            _row, _stack, scope, config = self.rows[label]
            await self.unload(label)
            await self.add(label, dict(config, update=True), scope)
        else:
            self.test.fail(f"{what}: {sorted(step)} is not a step the testing tool runs")
        await self.check(what, expect, outcome)

    async def check(self, what, expect, outcome):
        test = self.test
        if "returns" in expect:
            ok, value = outcome
            test.assertTrue(ok, f"{what}: expected {expect['returns']!r}, raised {value!r}")
            test.assertTrue(weather.same(value, expect["returns"]), f"{what}: expected {expect['returns']!r}, got {value!r}")
        if "throws" in expect:
            ok, value = outcome
            test.assertFalse(ok, f"{what}: expected an error, got {value!r}")
            self.error(what, value, expect["throws"])
        log = self.log.entries
        checks = {
            "log": lambda wanted: log == wanted,
            "logUnordered": lambda wanted: Counter(log) == Counter(wanted),
            "logContains": lambda wanted: contains(log, wanted),
        }
        for key, holds in checks.items():
            if key in expect:
                await self.until(lambda: holds(expect[key]))
                test.assertTrue(holds(expect[key]), f"{what}: {key} {expect[key]!r}, the log is {log!r}")
        for entry in expect.get("logLacks", []):
            test.assertNotIn(entry, log, f"{what}: the log is {log!r}")
        if "filterSees" in expect:
            test.assertEqual([seen["row"] for seen in self.log.seen], expect["filterSees"], f"{what}: filterSees")

    def error(self, what, error, spec):
        test = self.test
        if "name" in spec:
            test.assertEqual(name_of(error), spec["name"], f"{what}: the error's name ({error!r})")
        if "message" in spec:
            test.assertIn(spec["message"], message_of(error), f"{what}: the error's message")
        if "errors" in spec:
            children = getattr(error, "errors", None)
            test.assertIsInstance(children, list, f"{what}: an AggregateError's children")
            test.assertEqual(len(children), len(spec["errors"]), f"{what}: the children {children!r}")
            for i, (child, wanted) in enumerate(zip(children, spec["errors"])):
                self.error(f"{what} errors[{i}]", child, wanted)

    async def until(self, holds):
        """Wait up to SETTLE for the log to be complete."""
        deadline = asyncio.get_running_loop().time() + SETTLE
        while not holds() and asyncio.get_running_loop().time() < deadline:
            await asyncio.sleep(0.02)


class EventsConformance(background.TestCase):
    """One test per case the testing tool runs."""


def _case_test(case):
    def test(self):
        why = excluded(case)
        if why is not None:
            self.skipTest(f"not for {LANGUAGE}: {why}")
        asyncio.run(Run(self, case).run())

    test.__doc__ = f"{case['case']} (§{case['rule']})"
    return test


for _case in EXPECTED["cases"]:
    if _case.get("tool", True) is not False:
        setattr(EventsConformance, "test_" + re.sub(r"\W", "_", _case["case"]), _case_test(_case))


if __name__ == "__main__":
    unittest.main()

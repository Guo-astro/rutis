"""The event expectations (#464, docs/design-events-protocol-2026-10-18.md
§13) against the testing tool: every case the tool runs (not `tool:
false`, not limited to other languages), with every participant in this
process, through the plugin conformance-events, as the real runtimes are
checked against the same list.

Until crates/rutis-bridge/src/runtime/testing/events.expected.json is in
the checkout, its starting cases are read from the specification, whose
§13.3 is the file's first content.

`prepend` and `global` exist in every language (a maintainer's decision
after the specification): a case limited to other languages only because
of a listener's `options` runs here too.
"""

import asyncio
import inspect
import json
import time
import unittest
from contextlib import AsyncExitStack
from pathlib import Path

import background
import conformance_events
from test_conformance import same

from rutis import _values
from rutis.testing import Bus, load

ROOT = Path(__file__).parents[3]
FILE = ROOT / "crates/rutis-bridge/src/runtime/testing/events.expected.json"
SPEC = ROOT / "docs/design-events-protocol-2026-10-18.md"
# How long the log may take to be complete (§13.2).
SETTLE = 5.0


def expectations() -> dict:
    if FILE.exists():
        return json.loads(FILE.read_text("utf-8"))
    text = SPEC.read_text("utf-8")
    text = text[text.index("### 13.3"):]
    start = text.index("```json\n") + len("```json\n")
    return json.loads(text[start:text.index("\n```", start)])


def for_python(case: dict) -> str | None:
    """Why the tool skips `case`, or None."""
    if case.get("tool") is False:
        return "tool: false"
    optioned = {listener["label"] for listener in case.get("listeners", []) if listener.get("options")}
    for item in case.get("only", []):
        for key, languages in item.items():
            if key == "why" or "python" in languages:
                continue
            if key in optioned:
                continue
            return f"only {key}: {languages}"
    for item in case.get("except", []):
        for key, languages in item.items():
            if key != "why" and "python" in languages:
                return f"except {key}: {item.get('why')}"
    return None


def decode(value):
    """A value as the expectations write it: tagged values, callbacks."""
    if isinstance(value, list):
        return [decode(item) for item in value]
    if isinstance(value, dict):
        if len(value) == 1:
            (key, item), = value.items()
            if key == "$callback":
                if item == "upper":
                    return lambda text: text.upper()
                raise ValueError(f"unknown callback {item}")
            if key == "$map":
                return _values.received_map([decode(k) for k, _ in item], [decode(v) for _, v in item])
            if key == "$set":
                return {decode(element) for element in item}
            if key.startswith("$") and key[1:] in _values.TAGS:
                return _values.read(key[1:], item)
        return {key: decode(item) for key, item in value.items()}
    return value


class Log:
    """The host's service events-log."""

    def __init__(self):
        self.entries = []

    def add(self, entry):
        self.entries.append(entry)


def error_name(error) -> str:
    return getattr(error, "name", type(error).__name__)


def error_message(error) -> str:
    return getattr(error, "message", str(error))


class Case:
    def __init__(self, test, case, doc):
        self.test = test
        self.case = case
        self.bus = Bus(scopes=doc["scopes"])
        self.log = Log()
        self.rows = {}
        self.participants = {}
        self.generation = 0
        self.emitters = {}
        self.background = None
        modes = [step["emit"]["mode"] for step in case["steps"] if "emit" in step]
        self.kind = "sync" if modes and all(mode in ("bail", "waterfall-sync") for mode in modes) else "async"

    async def load(self, stack, participant: dict):
        config = dict(participant)
        config.setdefault("name", self.case["name"])
        config.setdefault("kind", self.kind)
        label = participant["label"]
        self.participants[label] = config
        self.rows[label] = await stack.enter_async_context(
            load(conformance_events, config=config, services={"events-log": self.log}, bus=self.bus,
                 row=label, scope=participant.get("scope", "root"))
        )

    async def emitter(self, stack, spec: dict):
        label, scope = spec.get("row", "e"), spec.get("scope", "root")
        key = (label, scope)
        if key not in self.emitters:
            row = label if not any(known == label for known, _ in self.emitters) else f"{label}@{scope}"
            self.emitters[key] = await stack.enter_async_context(
                load(conformance_events, config={"label": label, "emitter": True}, services={"events-log": self.log},
                     bus=self.bus, row=row, scope=scope)
            )
        return self.emitters[key].service(f"emitter-{label}")

    async def dispatch(self, stack, spec: dict):
        spec = dict(spec)
        spec.setdefault("name", self.case["name"])
        spec["args"] = decode(spec.get("args", []))
        service = await self.emitter(stack, spec)
        try:
            if spec["mode"] == "bail":
                # Run at the call, synchronously; an undeclared service's
                # method answers through an awaitable all the same.
                result = service.dispatch_sync(spec)
                return "return", await result if inspect.isawaitable(result) else result
            return "return", await service.dispatch(spec)
        except Exception as error:  # noqa: BLE001 - what the dispatch raised is the outcome
            return "throw", error

    async def run(self):
        async with AsyncExitStack() as stack:
            for participant in self.case.get("listeners", []):
                await self.load(stack, participant)
            for i, step in enumerate(self.case["steps"]):
                with self.test.subTest(case=self.case["case"], step=i):
                    await self.step(stack, step)
            if self.background is not None:
                await asyncio.wait_for(self.background, SETTLE)

    async def step(self, stack, step):
        outcome = None
        started = time.monotonic()
        if "emit" in step:
            dispatched = asyncio.ensure_future(self.dispatch(stack, step["emit"]))
            if step.get("background"):
                self.background = dispatched
                return
            outcome = await asyncio.wait_for(dispatched, SETTLE)
        elif "update" in step:
            # The row's config changes (a new generation): it registers again.
            self.generation += 1
            label = step["update"]
            await self.rows[label].update({**self.participants[label], "generation": self.generation})
        elif "load" in step:
            await self.load(stack, step["load"])
        elif "unload" in step:
            if "when" in step:
                await self.until(lambda: step["when"] in self.log.entries, f"{step['when']} started")
            await asyncio.wait_for(self.rows[step["unload"]].unload(), SETTLE)
            if self.background is not None:
                outcome = await asyncio.wait_for(self.background, SETTLE)
                self.background = None
        else:
            raise unittest.SkipTest(f"step {step} is not the testing tool's")
        expect = step.get("expect")
        if expect:
            await self.check(expect, outcome, time.monotonic() - started)

    async def until(self, check, what):
        deadline = time.monotonic() + SETTLE
        while not check():
            if time.monotonic() > deadline:
                self.test.fail(f"{self.case['case']}: {what} within {SETTLE}s; log {self.log.entries}")
            await asyncio.sleep(0.005)

    async def check(self, expect, outcome, took):
        t, case = self.test, self.case["case"]
        log = self.log.entries
        if "settles" in expect:
            t.assertLess(took, SETTLE, case)
        if "returns" in expect:
            t.assertIsNotNone(outcome, case)
            t.assertEqual(outcome[0], "return", f"{case}: {outcome}")
            t.assertTrue(same(outcome[1], expect["returns"]), f"{case}: returned {outcome[1]!r}, expected {expect['returns']!r}")
        if "throws" in expect:
            t.assertIsNotNone(outcome, case)
            t.assertEqual(outcome[0], "throw", f"{case}: {outcome}")
            error, thrown = outcome[1], expect["throws"]
            if "name" in thrown:
                t.assertEqual(error_name(error), thrown["name"], f"{case}: {error!r}")
            if "message" in thrown:
                t.assertIn(thrown["message"], error_message(error), case)
            if "errors" in thrown:
                children = getattr(error, "errors", [])
                t.assertEqual(len(children), len(thrown["errors"]), f"{case}: {children}")
                for child, wanted in zip(children, thrown["errors"]):
                    if "name" in wanted:
                        t.assertEqual(error_name(child), wanted["name"], case)
                    if "message" in wanted:
                        t.assertIn(wanted["message"], error_message(child), case)
        if "log" in expect:
            await self.until(lambda: len(log) >= len(expect["log"]), f"the log {expect['log']}")
            t.assertEqual(log, expect["log"], case)
        if "logUnordered" in expect:
            await self.until(lambda: len(log) >= len(expect["logUnordered"]), f"the log {expect['logUnordered']}")
            t.assertEqual(sorted(log), sorted(expect["logUnordered"]), case)
        if "logContains" in expect:
            def contains():
                it = iter(log)
                return all(any(entry == wanted for entry in it) for wanted in expect["logContains"])

            await self.until(contains, f"the log containing {expect['logContains']}")
        if "logLacks" in expect:
            for entry in expect["logLacks"]:
                t.assertNotIn(entry, log, case)
        if "filterSees" in expect:
            seen = [entry[len("filter:"):] for entry in log if entry.startswith("filter:")]
            t.assertEqual(seen, expect["filterSees"], case)
        if "reported" in expect:
            wanted = [(item["label"], item["name"]) for item in expect["reported"]]

            def reported():
                got = [(report.row, error_name(report.error)) for report in self.bus.reported]
                return all(item in got for item in wanted)

            await self.until(reported, f"the reports {wanted}")


class EventExpectations(background.TestCase):
    def test_the_testing_tool_meets_the_event_expectations(self):
        doc = expectations()
        self.assertEqual(doc["plugin"], "conformance-events")
        ran = []
        for case in doc["cases"]:
            why = for_python(case)
            if why is not None:
                print(f"{case['case']}: not run by the testing tool ({why})")
                continue
            with self.subTest(case=case["case"]):
                conformance_events._logged.clear()
                asyncio.run(Case(self, case, doc).run())
                ran.append(case["case"])
        self.assertIn("parallel/errors-in-registration-order", ran)
        self.assertIn("unload/waits-and-cancels", ran)


if __name__ == "__main__":
    unittest.main()

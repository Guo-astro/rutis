"""The event expectations (#464, docs/design-events-protocol-2026-10-18.md
§13) against the testing tool: every case the tool runs (not `tool:
false`, and `only` / `except` as for `python`), with every participant in
this process, through the plugin conformance-events, as the real runtimes
are checked against the same list. What each expectation means is
crates/rutis-bridge/src/runtime/testing/events.rs.

Until crates/rutis-bridge/src/runtime/testing/events.expected.json is in
the checkout, its starting cases are read from the specification, whose
§13.3 is the file's first content.
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
LANGUAGE = "python"
# How long the log may take to be complete (§13.2; events.rs SETTLE).
SETTLE = 5.0
# How long `logLacks` / `reported` wait after the rest held (events.rs LACKS).
LACKS = 0.3


def expectations() -> dict:
    if FILE.exists():
        return json.loads(FILE.read_text("utf-8"))
    text = SPEC.read_text("utf-8")
    text = text[text.index("### 13.3"):]
    start = text.index("```json\n") + len("```json\n")
    return json.loads(text[start:text.index("\n```", start)])


def emits_of(step: dict) -> list:
    """The emits of a step: itself, or its concurrent ones."""
    if "emit" in step:
        return [step["emit"]]
    return [spec for inner in step.get("concurrent", []) for spec in emits_of(inner)]


def labels_of(case: dict) -> set:
    """The labels of a case (events.rs `places`): its listeners, its loaded
    participants and its emitter rows."""
    labels = {listener["label"] for listener in case.get("listeners", [])}
    for step in case["steps"]:
        if "load" in step:
            labels.add(step["load"]["label"])
        labels.update(spec.get("row", "e") for spec in emits_of(step))
    return labels


def holds(limit: dict, labels: set) -> bool:
    """Whether a limit holds with every label at X in Python (events.rs
    `limit_holds`): every label it names (X, Y or the case's) is of one of
    its languages."""
    return all(
        (label in ("X", "Y") or label in labels) and LANGUAGE in languages
        for label, languages in limit.items()
        if label != "why"
    )


def for_python(case: dict) -> str | None:
    """Why the tool skips `case`, or None (events.rs `tool_combo`)."""
    if case.get("tool") is False:
        return "tool: false"
    labels = labels_of(case)
    for limit in case.get("only", []):
        if not holds(limit, labels):
            return f"only: {limit.get('why')}"
    for limit in case.get("except", []):
        if holds(limit, labels):
            return f"except: {limit.get('why')}"
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


def reports(line: str, event: str, filter_: bool, row: str) -> str | None:
    """The error name of a §5.6 report `line` about `event` naming `row`, or
    None when the line is about something else (events.rs `reports`):
    `rutis: listener of <event> in row <row> failed: <Name>: <message>`, or
    `rutis: filter of <event> from row <row> failed: ...`."""
    kind, at = ("filter", "from") if filter_ else ("listener", "in")
    prefix = f"rutis: {kind} of {event} {at} row {row} failed: "
    if not line.startswith(prefix):
        return None
    name, colon, _message = line[len(prefix):].partition(": ")
    return name if colon else None


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
        modes = [spec["mode"] for step in case["steps"] for spec in emits_of(step)]
        self.kind = "sync" if modes and all(mode == "bail" for mode in modes) else "async"

    async def load(self, stack, participant: dict):
        config = {key: value for key, value in participant.items() if key not in ("at", "scope")}
        config.setdefault("name", self.case["name"])
        config.setdefault("kind", self.kind)
        config.setdefault("log", "label")
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
        service = await self.emitter(stack, spec)
        # The spec as the emitter's dispatch receives it (events.rs `Spec::wire`).
        wire = {key: value for key, value in spec.items() if key not in ("from", "row", "scope")}
        wire.setdefault("name", self.case["name"])
        wire["args"] = decode(spec.get("args", []))
        try:
            if wire["mode"] == "bail":
                # Run at the call, synchronously; an undeclared service's
                # method answers through an awaitable all the same.
                result = service.dispatch_sync(wire)
                return "return", await result if inspect.isawaitable(result) else result
            return "return", await service.dispatch(wire)
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
        outcomes = []
        started = time.monotonic()
        expect = step.get("expect") or {}
        emits = emits_of(step)
        event = (emits[0].get("name") if emits else None) or self.case["name"]
        if "emit" in step:
            dispatched = asyncio.ensure_future(self.dispatch(stack, step["emit"]))
            if step.get("background"):
                self.background = dispatched
                await self.check_log(expect, event)
                return
            outcomes.append(await asyncio.wait_for(dispatched, SETTLE))
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
                outcomes.append(await asyncio.wait_for(self.background, SETTLE))
                self.background = None
        else:
            raise unittest.SkipTest(f"step {step} is not the testing tool's")
        if "settles" in expect:
            self.test.assertLess(time.monotonic() - started, SETTLE, self.case["case"])
        self.check_outcomes(expect, outcomes)
        await self.check_log(expect, event)

    async def until(self, check, what):
        deadline = time.monotonic() + SETTLE
        while not check():
            if time.monotonic() > deadline:
                self.test.fail(f"{self.case['case']}: {what} within {SETTLE}s; log {self.log.entries}")
            await asyncio.sleep(0.005)

    def check_outcomes(self, expect, outcomes):
        """`returns` and `throws` against the dispatches that end in the
        step (events.rs `check_outcomes`): without `throws` every one must
        succeed."""
        t, case = self.test, self.case["case"]
        if expect.get("cancelled"):
            t.fail(f"{case}: cancelled: the testing tool does not give up a dispatch")
        if ("returns" in expect or "throws" in expect) and not outcomes:
            t.fail(f"{case}: expects a result, but no dispatch ends in this step")
        for outcome in outcomes:
            if "throws" not in expect:
                t.assertEqual(outcome[0], "return", f"{case}: expected the dispatch to succeed: {outcome}")
            if "returns" in expect:
                t.assertEqual(outcome[0], "return", f"{case}: {outcome}")
                t.assertTrue(same(outcome[1], expect["returns"]),
                             f"{case}: returned {outcome[1]!r}, expected {expect['returns']!r}")
        if "throws" in expect:
            thrown = [outcome[1] for outcome in outcomes if outcome[0] == "throw"]
            t.assertTrue(thrown, f"{case}: should throw: {outcomes}")
            error, wanted = thrown[0], expect["throws"]
            if "name" in wanted:
                t.assertEqual(error_name(error), wanted["name"], f"{case}: {error!r}")
            if "message" in wanted:
                t.assertIn(wanted["message"], error_message(error), case)
            if "errors" in wanted:
                children = getattr(error, "errors", [])
                t.assertEqual(len(children), len(wanted["errors"]), f"{case}: {children}")
                for child, sub in zip(children, wanted["errors"]):
                    if "name" in sub:
                        t.assertEqual(error_name(child), sub["name"], case)
                    if "message" in sub:
                        t.assertIn(sub["message"], error_message(child), case)

    def log_problem(self, expect, event) -> str | None:
        """What of the step's log expectations does not hold now, or None
        (events.rs `log_holds`)."""
        log = list(self.log.entries)
        if "log" in expect and log != expect["log"]:
            return f"log: expected {expect['log']}, got {log}"
        if "logUnordered" in expect and sorted(log) != sorted(expect["logUnordered"]):
            return f"log (any order): expected {expect['logUnordered']}, got {log}"
        if "logContains" in expect:
            it = iter(log)
            if not all(any(entry == wanted for entry in it) for wanted in expect["logContains"]):
                return f"log: expected to contain {expect['logContains']} in order, got {log}"
        for entry in expect.get("logLacks", []):
            if entry in log:
                return f"log: {entry!r} must not be logged: {log}"
        if "filterSees" in expect:
            seen = [entry[len("filter:"):] for entry in log if entry.startswith("filter:")]
            if seen != expect["filterSees"]:
                return f"filterSees: expected {expect['filterSees']}, the filter saw {seen}"
        if "reported" in expect:
            lines = [str(report) for report in self.bus.reported]
            for item in expect["reported"]:
                label, name, filter_ = item["label"], item["name"], item.get("filter", False)
                about = [line for line in lines if reports(line, event, filter_, label) is not None]
                if len(about) != 1:
                    return f"reported: expected one line for {label} about {event}, got {len(about)}: {lines}"
                if reports(about[0], event, filter_, label) != name:
                    return f"reported: {label} should fail with {name}: {about[0]!r}"
        return None

    async def check_log(self, expect, event):
        """The log expectations until they hold; with `logLacks` or
        `reported`, once more a moment later (events.rs `check_log`)."""
        deadline = time.monotonic() + SETTLE
        while (problem := self.log_problem(expect, event)) is not None:
            if time.monotonic() > deadline:
                self.test.fail(f"{self.case['case']}: {problem}")
            await asyncio.sleep(0.005)
        if "logLacks" in expect or "reported" in expect:
            await asyncio.sleep(LACKS)
            problem = self.log_problem(expect, event)
            if problem is not None:
                self.test.fail(f"{self.case['case']}: {problem} (after the step had settled)")


# Cases a product bug fails here, by case: each skipped with its reason
# (counted, so that the tool still runs as many as the file says).
KNOWN_FAILING: dict = {}


class EventExpectations(background.TestCase):
    def test_the_testing_tool_meets_the_event_expectations(self):
        doc = expectations()
        self.assertEqual(doc["plugin"], "conformance-events")
        ran = []
        known = []
        for case in doc["cases"]:
            why = for_python(case)
            if why is not None:
                print(f"{case['case']}: not run by the testing tool ({why})")
                continue
            if case["case"] in KNOWN_FAILING:
                print(f"{case['case']}: known failing ({KNOWN_FAILING[case['case']]})")
                known.append(case["case"])
                continue
            with self.subTest(case=case["case"]):
                conformance_events._logged.clear()
                asyncio.run(Case(self, case, doc).run())
                ran.append(case["case"])
        # The skip logic agrees with events.rs `tool_combo`.
        self.assertEqual(len(ran) + len(known), doc["toolCases"][LANGUAGE], f"ran {ran}, known failing {known}")


if __name__ == "__main__":
    unittest.main()

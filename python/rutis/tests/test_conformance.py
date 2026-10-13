"""The testing tool against the real runtime: the same plugin, the same
calls, the same expected results
(crates/rutis-bridge/src/runtime/testing/weather.expected.json), which
rutis_bridge::runtime::testing::runtime checks on the real Python runtime.

guarantee: sdk/testing-matches-runtime (risk A1, Q6.7.2), the testing tool's
side. Entries marked `allowed` are the written-down differences
(docs/guide/plugin-api.md); the test says which it skips and why.
"""

import asyncio
import json
import unittest
from pathlib import Path

import background
import conformance_weather

from rutis import plugin as sdk
from rutis.peer import RemoteError
from rutis.testing import load

EXPECTED = json.loads(
    (Path(__file__).parents[3] / "crates/rutis-bridge/src/runtime/testing/weather.expected.json").read_text("utf-8")
)


class Clock:
    """The clock the plugin injects: 0, 1, 2, ..."""

    def __init__(self):
        self.next = 0

    def now(self):
        self.next += 1
        return self.next - 1


def argument(arg):
    """An argument as the expectations write it: data, or a callback."""
    callback = arg.get("$callback") if isinstance(arg, dict) else None
    if callback is None:
        return arg
    if callback == "upper":
        return lambda day: day.upper()
    spec = callback["throws"]
    error = type(spec["name"], (Exception,), {})

    def throw(*args):
        raised = error(spec["message"])
        for field, value in spec.get("fields", {}).items():
            setattr(raised, field, value)
        raise raised

    return throw


def same(a, b):
    """JSON equality that tells apart what JSON does: bool from int, list
    from tuple, float from int except where JSON cannot."""
    if isinstance(b, bool) or b is None:
        return a is b
    if isinstance(b, (int, float)):
        return type(a) in (int, float) and not isinstance(a, bool) and a == b
    if isinstance(b, str):
        return type(a) is str and a == b
    if isinstance(b, list):
        return type(a) is list and len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    if isinstance(b, dict):
        return type(a) is dict and a.keys() == b.keys() and all(same(a[key], b[key]) for key in b)
    raise TypeError(f"not JSON: {b!r}")


class Conformance(background.TestCase):
    def check(self, what, entry, make, args=()):
        why = f"{what} ({entry.get('why')})"
        try:
            returned, raised = make(), None
        except Exception as error:  # noqa: BLE001 - what the call raised is the result
            returned, raised = None, error
        if "returns" in entry:
            self.assertIsNone(raised, f"{why}: expected {entry['returns']!r}, raised {raised!r}")
            compared = returned
            if isinstance(returned, dict) and "ignore" in entry:
                compared = {key: value for key, value in returned.items() if key not in entry["ignore"]}
            self.assertTrue(same(compared, entry["returns"]), f"{why}: expected {entry['returns']!r}, got {returned!r}")
            if entry.get("copied"):
                self.assertTrue(all(arg is not returned for arg in args), f"{why}: a copy, not the object passed in")
            return
        self.assertIsNotNone(raised, f"{why}: expected an error, got {returned!r}")
        name = raised.name if isinstance(raised, RemoteError) else type(raised).__name__
        message = raised.message if isinstance(raised, RemoteError) else str(raised)
        if "name" in entry["throws"]:
            self.assertEqual(name, entry["throws"]["name"], f"{why}: the error's name")
        if "message" in entry["throws"]:
            self.assertEqual(message, entry["throws"]["message"], f"{why}: the error's message")
        # What crosses is the name and message, not the object raised.
        for arg in entry.get("args", []):
            fields = arg.get("$callback", {}).get("throws", {}).get("fields", {}) if isinstance(arg, dict) else {}
            for field in fields:
                self.assertFalse(hasattr(raised, field), f"{why}: {field} does not cross")

    def test_the_testing_tool_returns_what_the_real_runtime_does(self):
        async def go():
            async with load(conformance_weather, services={"clock": Clock()}, emits=EXPECTED["emits"]) as t:
                service = t.service(EXPECTED["service"])
                shape = sdk.load(conformance_weather).provides[EXPECTED["service"]]
                for method, kind in EXPECTED["methods"].items():
                    self.assertEqual(shape.get(method), kind, f"the plugin declares {method} as {kind}")
                for i, entry in enumerate(EXPECTED["calls"]):
                    what = f"calls[{i}] {entry['call']}"
                    if "allowed" in entry:
                        print(f"{what}: not made here, an allowed difference: {entry['allowed']}")
                        continue
                    args = [argument(arg) for arg in entry["args"]]
                    method = getattr(service, entry["call"])
                    if shape[entry["call"]] == "async":
                        make = await _settled(method(*args))
                    else:
                        make = lambda method=method, args=args: method(*args)  # noqa: E731
                    with self.subTest(what):
                        self.check(what, entry, make, args)
                names = service.samples()
                self.assertTrue(names, "the Python plugin has samples")
                for name in names:
                    entry = next((entry for entry in EXPECTED["samples"] if entry["sample"] == name), None)
                    self.assertIsNotNone(entry, f"the expectations list sample {name}")
                    if "allowed" in entry:
                        print(f"sample {name}: an allowed difference: {entry['allowed']}")
                        continue
                    with self.subTest(f"sample {name}"):
                        self.check(f"sample {name}", entry, lambda name=name: service.sample(name))
                for entry in EXPECTED["events"]:
                    with self.subTest(f"event {entry['emit']}"):
                        emitted = await _settled(t.emit(entry["emit"], *entry["args"]))
                        if "throws" in entry:
                            self.check("event", entry, emitted)
                        else:
                            emitted()
                            call = entry["then"]
                            self.check("after event", call, lambda: getattr(service, call["call"])(*call["args"]))

        asyncio.run(go())


async def _settled(awaitable):
    """What awaiting `awaitable` does, as a function that does it again."""
    try:
        value = await awaitable
    except Exception as error:  # noqa: BLE001 - what the call raised is the result
        def again(error=error):
            raise error

        return again
    return lambda: value


if __name__ == "__main__":
    unittest.main()

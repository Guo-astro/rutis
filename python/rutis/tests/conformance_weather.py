"""The runtime conformance plugin (rutis_bridge::runtime::testing::runtime),
for the Python runtime. What calls on it return is in
crates/rutis-bridge/src/runtime/testing/weather.expected.json; the real
runtime and the testing tool (rutis.testing, tests/test_conformance.py) are
both checked against it."""
import os
from dataclasses import dataclass

inject = ["clock"]


@dataclass
class Reading:
    city: str
    degrees: int


# Values only Python makes, by name (`samples` in the expectations).
SAMPLES = {
    "python/tuple": lambda: (1, "a"),
    "python/dataclass": lambda: Reading("Oslo", 21),
    "python/set": lambda: {1},
    "python/bytes": lambda: b"x",
    "python/nan": lambda: float("nan"),
    "python/int-keys": lambda: {1: "a"},
    "python/exception-value": lambda: ValueError("bad"),
    "python/safe-int": lambda: [2**53 - 1, -(2**53 - 1), True],
    "python/big-int": lambda: 2**64 + 1,
    "python/big-int-nested": lambda: {"n": [1, -(2**53)]},
}


class Weather:
    def __init__(self, clock):
        self.clock = clock

    def today(self):
        return f"Oslo at {self.clock.now()}"

    async def later(self):
        return "Oslo later"

    def each(self, callback):
        return [callback(day) for day in ("mon", "tue")]

    def echo(self, value):
        return value

    def crash(self):
        os._exit(17)

    def samples(self):
        return list(SAMPLES)

    def sample(self, name):
        return SAMPLES[name]()


provides = {"weather": Weather}


def apply(ctx, config):
    ctx.provide("weather", Weather(ctx.use("clock")))

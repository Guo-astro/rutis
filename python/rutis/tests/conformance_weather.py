"""The runtime conformance plugin (rutis_bridge::runtime::testing::runtime),
for the Python runtime. What calls on it return is in
crates/rutis-bridge/src/runtime/testing/weather.expected.json; the real
runtime and the testing tool (rutis.testing, tests/test_conformance.py) are
both checked against it."""
import os
from dataclasses import dataclass
from datetime import date, datetime, time, timedelta, timezone
from decimal import Decimal
from uuid import UUID

from rutis import BigInt, Map
from rutis.peer import UNDEFINED

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
    "python/undefined-in-data": lambda: {"a": UNDEFINED, "b": [UNDEFINED]},
    "python/value-instant": lambda: datetime(2026, 10, 15, 5, 22, 15, 123456, tzinfo=timezone(timedelta(hours=2))),
    "python/value-naive-datetime": lambda: datetime(2026, 10, 15, 14, 30, 5, 250000),
    "python/value-decimal-uuid": lambda: [Decimal("-12.3400"), UUID("0F8FAD5B-D9CB-469F-A165-70867728950E")],
    "python/value-int-keys": lambda: {1: "a", 2: "b"},
    "python/value-date-time": lambda: [date(2026, 10, 15), time(14, 30, 5, 250000)],
    "python/value-bigint": lambda: BigInt(-123456789012345678901),
    "python/value-str-map": lambda: Map({"a": 1, "b": [frozenset()]}),
    "python/value-bytes-kinds": lambda: [bytearray(b"hello"), memoryview(b"")],
    "python/value-aware-time": lambda: time(14, 30, tzinfo=timezone.utc),
    "python/value-decimal-nan": lambda: Decimal("NaN"),
    "python/value-tuple-key": lambda: {(1, 2): "a"},
}


class Weather:
    def __init__(self, clock):
        self.clock = clock
        self.city = "Oslo"

    def today(self):
        return f"{self.city} at {self.clock.now()}"

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
    weather = Weather(ctx.use("clock"))
    ctx.on("weather/city", lambda city: setattr(weather, "city", city))
    ctx.provide("weather", weather)

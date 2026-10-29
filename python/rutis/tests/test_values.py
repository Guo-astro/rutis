"""Tagged values (#308, docs/design-value-types-2026-10-15.md): their wire
formats, as strict as the Rust reference (crates/rutis-bridge/src/session/
values.rs); map keys and set elements (design §5); and the session: sent
only to a far end that declared `value-<tag>`, a refused one fails only
its call and releases the references it carried; datetime, Decimal and
UUID cross by value, not by reference (#355)."""

import asyncio
import json
import socket
import unittest
from datetime import date, datetime, time, timedelta, timezone
from decimal import Decimal
from uuid import UUID

import background
from support import LOCAL, hello

from rutis import _values as values
from rutis._values import BigInt, Map
from rutis.peer import CAPABILITIES, UNDEFINED, Peer, RemoteFunction

ALL = [f"value-{tag}" for tag in values.TAGS]


class FormatTests(unittest.TestCase):
    def round_trip(self, tag, value, text):
        self.assertEqual(values.tag_of(value), tag)
        self.assertEqual(values.write(tag, value), text)
        self.assertEqual(values.read(tag, text), value)

    def test_each_tag_writes_and_reads_its_text(self):
        self.round_trip("instant", datetime(2026, 10, 15, 3, 22, 15, 123456, tzinfo=timezone.utc), "2026-10-15T03:22:15.123456Z")
        self.round_trip("instant", datetime(1, 1, 1, tzinfo=timezone.utc), "0001-01-01T00:00:00Z")
        self.round_trip("datetime", datetime(2026, 10, 15, 14, 30, 5, 250000), "2026-10-15T14:30:05.25")
        self.round_trip("date", date(2026, 10, 15), "2026-10-15")
        self.round_trip("date", date(2000, 2, 29), "2000-02-29")
        self.round_trip("time", time(0, 0), "00:00:00")
        self.round_trip("time", time(23, 59, 59, 999999), "23:59:59.999999")
        self.round_trip("bytes", b"hello", "aGVsbG8=")
        self.round_trip("bytes", b"", "")
        self.round_trip("bigint", BigInt(-123456789012345678901), "-123456789012345678901")
        self.round_trip("bigint", BigInt(0), "0")
        self.round_trip("bigint", BigInt(int("9" * 4300)), "9" * 4300)
        self.round_trip("decimal", Decimal("-12.3400"), "-12.3400")
        self.round_trip("decimal", Decimal("-0"), "-0")
        self.round_trip("uuid", UUID("0f8fad5b-d9cb-469f-a165-70867728950e"), "0f8fad5b-d9cb-469f-a165-70867728950e")

    def test_an_instant_is_written_in_utc(self):
        plus_two = timezone(timedelta(hours=2))
        self.assertEqual(values.write("instant", datetime(2026, 10, 15, 5, 22, 15, tzinfo=plus_two)), "2026-10-15T03:22:15Z")
        with self.assertRaisesRegex(ValueError, "outside years 0001–9999"):
            values.write("instant", datetime(1, 1, 1, tzinfo=plus_two))

    def test_received_times_keep_microseconds_truncated_toward_negative_infinity(self):
        # Design V7: Python holds microseconds.
        self.assertEqual(
            values.read("instant", "1969-12-31T23:59:59.999999999Z"),
            datetime(1969, 12, 31, 23, 59, 59, 999999, tzinfo=timezone.utc),
        )
        self.assertEqual(
            values.read("instant", "9999-12-31T23:59:59.999999999Z"),
            datetime(9999, 12, 31, 23, 59, 59, 999999, tzinfo=timezone.utc),
        )
        self.assertEqual(values.read("datetime", "2026-10-15T14:30:05.1234567"), datetime(2026, 10, 15, 14, 30, 5, 123456))
        self.assertEqual(values.read("time", "14:30:05.000000999"), time(14, 30, 5))

    def test_a_decimal_is_written_without_an_exponent_and_keeps_its_trailing_zeros(self):
        for value, text in (
            (Decimal("1.10"), "1.10"),
            (Decimal("1E+3"), "1000"),
            (Decimal("1.2E-7"), "0.00000012"),
            (Decimal("0E-3"), "0.000"),
            (Decimal("-0.00"), "-0.00"),
        ):
            self.assertEqual(values.write("decimal", value), text)
        self.assertNotEqual(str(values.read("decimal", "1.10")), str(values.read("decimal", "1.1")))
        for value in (Decimal("NaN"), Decimal("sNaN"), Decimal("Infinity"), Decimal("-Infinity")):
            with self.assertRaisesRegex(ValueError, "only finite decimals"):
                values.write("decimal", value)
        for value in (Decimal("1E+4300"), Decimal("1E-4301"), Decimal("1" * 4301)):
            with self.assertRaisesRegex(ValueError, "more than 4300 digits"):
                values.write("decimal", value)

    def test_values_that_would_lose_what_they_are_are_not_written(self):
        with self.assertRaisesRegex(ValueError, "without a time zone"):
            values.write("time", time(14, 30, tzinfo=timezone.utc))
        with self.assertRaisesRegex(ValueError, "more than 4300 digits"):
            values.write("bigint", BigInt(10**4300))

    def test_text_not_in_the_format_is_refused(self):
        refused = {
            "instant": [
                "2026-10-15T03:22:15", "2026-10-15T03:22:15+00:00", "2026-10-15t03:22:15Z", "2026-10-15T03:22:15z",
                "2026-10-15 03:22:15Z", "2026-10-15T03:22:60Z", "2026-10-15T24:00:00Z", "0000-01-01T00:00:00Z",
                "2026-02-30T00:00:00Z", "2026-10-15T03:22:15.Z", "2026-10-15T03:22:15.1234567890Z",
                "2026-10-15T03:22:15Z\n", "２026-10-15T03:22:15Z", "+2026-10-15T03:22:15Z",
            ],
            "datetime": ["2026-10-15T14:30:05Z", "2026-10-15T14:30", "2026-10-15", "2026-13-01T00:00:00"],
            "date": ["2026-10-15T00:00:00", "2026-1-5", "2026-00-01", "2025-02-29", "0000-01-01", "2026-10-15\n"],
            "time": ["14:30", "24:00:00", "14:30:05Z", "14:30:05+01:00", "14:60:00", "1:30:05"],
            "bytes": ["aGVsbG8", "aGVsbG9=", "aGVs bG8=", "aGVsbG8=\n", "aGVsbG8==", "a===", "aG=sbG8=", "aGVsbG8-", "é"],
            "bigint": ["", "-", "-0", "01", "+1", "1.0", "1e3", " 1", "1" * 4301, "١"],
            "decimal": ["", ".5", "5.", "01.5", "+1", "1e3", "1E3", "NaN", "Infinity", "1" * 4301, "0." + "1" * 4300],
            "uuid": ["0F8FAD5B-D9CB-469F-A165-70867728950E", "0f8fad5bd9cb469fa16570867728950e", "{0f8fad5b-d9cb-469f-a165-70867728950e}", "0f8fad5b-d9cb-469f-a165-70867728950"],
        }
        for tag, texts in refused.items():
            for text in texts:
                with self.subTest(tag=tag, text=text):
                    with self.assertRaises(values.Refused):
                        values.read(tag, text)

    def test_tags_of_python_values(self):
        self.assertEqual(values.tag_of(bytearray(b"x")), "bytes")
        self.assertEqual(values.tag_of(memoryview(b"x")), "bytes")
        self.assertEqual(values.tag_of(frozenset()), "set")
        self.assertEqual(values.tag_of({1: "a"}), "map")
        self.assertEqual(values.tag_of(Map({"a": 1})), "map")
        self.assertEqual(values.tag_of(2**53), "bigint")
        self.assertEqual(values.tag_of(-(2**53)), "bigint")
        for plain in ({"a": 1}, {}, 2**53 - 1, True, 1.5, "x", None, [date(2026, 1, 1)]):
            self.assertIsNone(values.tag_of(plain))


class BigIntTests(unittest.TestCase):
    def test_a_bigint_is_an_int_whose_arithmetic_gives_plain_ints(self):
        n = BigInt(2**64)
        self.assertIsInstance(n, int)
        self.assertEqual(n, 2**64)
        self.assertIs(type(n + 1), int)
        self.assertIs(type(-n), int)
        self.assertEqual(repr(BigInt(-5)), "BigInt(-5)")
        self.assertEqual(str(BigInt(-5)), "-5")
        self.assertEqual(values.read("bigint", "5"), BigInt(5))
        self.assertIs(type(values.read("bigint", "5")), BigInt)


class KeyTests(unittest.TestCase):
    def test_keys_of_every_language(self):
        values.check_keys(
            ["a", 1, 1.5, True, BigInt(2**80), datetime(2026, 1, 1), date(2026, 1, 1), time(1, 2), Decimal("1.5"), UUID(int=1), 2**53 - 1],
            "map key",
        )

    def test_keys_no_language_takes_are_refused(self):
        for key in (
            (1, 2),
            None,
            b"x",
            datetime(2026, 1, 1, tzinfo=timezone.utc),
            frozenset(),
            float("nan"),
            float("inf"),
            Decimal("NaN"),
            time(1, 2, tzinfo=timezone.utc),
        ):
            with self.subTest(key=key):
                with self.assertRaises(ValueError):
                    values.check_keys([key], "map key")
        # Beyond ±(2^53−1) an int key is a bigint (design V6).
        self.assertEqual(values.key_of(2**53, "map key"), ("bigint", 2**53))
        values.check_keys([2**53, -(2**64)], "map key")

    def test_keys_equal_on_the_wire_are_refused(self):
        with self.assertRaisesRegex(ValueError, "duplicate map key 1"):
            values.check_keys([1, 1.0], "map key")
        with self.assertRaisesRegex(ValueError, "duplicate set element -0.0"):
            values.check_keys([0, -0.0], "set element")

    def test_received_keys_that_collide_in_python_are_refused(self):
        # Distinct on the wire (design §5), one key in a Python dict.
        for keys in ([1, True], [1, BigInt(1)], [1, Decimal("1.0")], [Decimal("1.0"), Decimal("1.00")]):
            with self.subTest(keys=keys):
                with self.assertRaisesRegex(values.Refused, "equal in Python"):
                    values.received_map(keys, ["a", "b"])
                with self.assertRaisesRegex(values.Refused, "equal in Python"):
                    values.received_set(keys)
        self.assertEqual(values.received_map(["1", 1], ["a", "b"]), {"1": "a", 1: "b"})
        self.assertIs(type(values.received_map([], [])), Map)
        # A number on the wire beyond ±(2^53−1) is no key; a bigint is.
        with self.assertRaisesRegex(values.Refused, "beyond"):
            values.received_map([2**53], ["a"])
        with self.assertRaisesRegex(values.Refused, "beyond"):
            values.received_set([-(2**53)])
        self.assertEqual(values.received_set([BigInt(2**53)]), {2**53})


class SessionTests(background.IsolatedAsyncioTestCase):
    def session(self, dispatch, capabilities=ALL):
        ours, theirs = socket.socketpair()
        theirs.settimeout(10)
        self.addCleanup(ours.close)
        self.addCleanup(theirs.close)
        peer = Peer(ours, dispatch, endpoint=LOCAL)
        self.addCleanup(peer._channel.close)
        self.addCleanup(peer.close)
        peer._receive(hello(capabilities))
        lines = theirs.makefile("rb")
        self.addCleanup(lines.close)

        async def read():
            return json.loads(await asyncio.to_thread(lines.readline))

        return peer, read

    @staticmethod
    def invoke(sequence, args, method="m"):
        return {"op": "invoke", "id": f"main:{sequence}", "path": [], "target": "test", "method": method, "args": args}

    def test_the_session_declares_every_tagged_value(self):
        for capability in ALL:
            self.assertIn(capability, CAPABILITIES)

    async def test_values_cross_by_value_under_their_tags(self):
        peer, _read = self.session(lambda target, method, args: None)
        sent = [
            datetime(2026, 10, 15, 3, 22, 15, tzinfo=timezone.utc),
            datetime(2026, 10, 15, 14, 30),
            Decimal("1.10"),
            UUID("0f8fad5b-d9cb-469f-a165-70867728950e"),
            {1: "a", "b": UNDEFINED},
            {"plain": {2}},
            Map({"k": b"\x00"}),
            BigInt(7),
        ]
        wire = peer._encode(sent, [], False, -1)
        self.assertEqual(peer._exports, {}, "nothing crosses by reference (#355)")
        self.assertEqual(
            wire,
            {
                "type": "list",
                "value": [
                    {"type": "instant", "value": "2026-10-15T03:22:15Z"},
                    {"type": "datetime", "value": "2026-10-15T14:30:00"},
                    {"type": "decimal", "value": "1.10"},
                    {"type": "uuid", "value": "0f8fad5b-d9cb-469f-a165-70867728950e"},
                    {
                        "type": "map",
                        "value": [
                            [{"type": "data", "value": 1}, {"type": "data", "value": "a"}],
                            [{"type": "data", "value": "b"}, {"type": "data", "value": None}],
                        ],
                    },
                    {"type": "record", "value": {"plain": {"type": "set", "value": [{"type": "data", "value": 2}]}}},
                    {"type": "map", "value": [[{"type": "data", "value": "k"}, {"type": "bytes", "value": "AA=="}]]},
                    {"type": "bigint", "value": "7"},
                ],
            },
        )
        decoded = peer._decode(wire)
        self.assertEqual(decoded[:4], sent[:4])
        self.assertEqual(decoded[4], {1: "a", "b": None})
        self.assertIs(type(decoded[4]), Map)
        self.assertEqual(decoded[5], {"plain": {2}})
        self.assertEqual(decoded[6], Map({"k": b"\x00"}))
        self.assertIs(type(decoded[7]), BigInt)

    async def test_an_int_beyond_the_safe_range_crosses_as_a_bigint(self):
        # Design V6: no wrapper; within the range it stays a number.
        peer, _read = self.session(lambda target, method, args: None)
        self.assertEqual(
            peer._encode({"n": [1, 2**64], 2**53: True}, [], False),
            {
                "type": "map",
                "value": [
                    [{"type": "data", "value": "n"}, {"type": "list", "value": [{"type": "data", "value": 1}, {"type": "bigint", "value": "18446744073709551616"}]}],
                    [{"type": "bigint", "value": "9007199254740992"}, {"type": "data", "value": True}],
                ],
            },
        )
        # A received bigint within the range passes on as a bigint.
        received = peer._decode({"type": "bigint", "value": "5"})
        self.assertEqual(peer._encode(received, [], False), {"type": "bigint", "value": "5"})
        self.assertEqual(peer._encode(received + 1, [], False), {"type": "data", "value": 6})

    async def test_undefined_inside_data_is_null_and_a_dict_keeps_its_key(self):
        # #355, design V10: as data, beside a tagged value, or in a map.
        peer, _read = self.session(lambda target, method, args: None)
        null = {"type": "data", "value": None}
        self.assertEqual(peer._encode({"a": UNDEFINED}, [], False), {"type": "data", "value": {"a": None}})
        self.assertEqual(
            peer._encode({"a": UNDEFINED, "b": [UNDEFINED, b""]}, [], False),
            {
                "type": "record",
                "value": {"a": null, "b": {"type": "list", "value": [null, {"type": "bytes", "value": ""}]}},
            },
        )
        # An argument list keeps an omitted argument.
        self.assertEqual(
            peer._encode([1, UNDEFINED], [], False, -1),
            {"type": "list", "value": [{"type": "data", "value": 1}, {"type": "undefined"}]},
        )

    async def test_a_dataclass_carries_tagged_values_but_no_references(self):
        import dataclasses

        @dataclasses.dataclass
        class Reading:
            at: date
            note: object = None

        peer, _read = self.session(lambda target, method, args: None)
        self.assertEqual(
            peer._encode([Reading(date(2026, 1, 2))], [], False, -1),
            {
                "type": "list",
                "value": [
                    {"type": "record", "value": {"at": {"type": "date", "value": "2026-01-02"}, "note": {"type": "data", "value": None}}}
                ],
            },
        )
        with self.assertRaisesRegex(TypeError, "a dataclass crosses as data"):
            peer._encode(Reading(date(2026, 1, 2), print), [], False)
        self.assertEqual(peer._exports, {})

    async def test_a_value_the_far_end_did_not_declare_fails_only_that_call(self):
        def callback():
            return 1

        peer, read = self.session(lambda target, method, args: None, capabilities=[])
        for value in (date(2026, 1, 1), {1: "a"}, [callback, {"at": Decimal(1)}], BigInt(1), {"a": {b"x"}}):
            with self.subTest(value=value):
                with self.assertRaisesRegex(ValueError, "did not declare the capability value-"):
                    peer.call("test", "m", [value])
        self.assertEqual(peer._exports, {}, "the partial grant is rolled back")
        peer._deliver({"op": "return", "id": "py:6", "value": {"type": "data", "value": 7}})
        self.assertEqual(peer.call("test", "good", [{"a": 1}]), 7)
        self.assertEqual((await read())["id"], "py:6")
        self.assertIsNone(peer.closed_error)

    async def test_a_result_the_far_end_did_not_declare_is_thrown(self):
        peer, read = self.session(lambda target, method, args: UUID(int=1), capabilities=["value-instant"])
        peer._receive(self.invoke(1, {"type": "data", "value": []}))
        reply = await read()
        self.assertEqual(reply["op"], "throw", reply)
        self.assertIn("did not declare the capability value-uuid", reply["error"]["message"])
        self.assertIsNone(peer.closed_error)

    async def test_refused_arguments_fail_that_call_and_release_their_references(self):
        # Review of #406: a refused call still imports, then releases, every
        # reference it carries; the session goes on.
        called = []
        peer, read = self.session(lambda target, method, args: called.append(args) or "ok")
        reference = lambda ref: {"type": "reference", "value": {"id": ref, "kind": "function", "home": False, "origin": []}}  # noqa: E731
        refused = [
            {"type": "list", "value": [{"type": "bytes", "value": "aGVsbG9="}, reference(1)]},
            {"type": "list", "value": [reference(2), {"type": "map", "value": [[{"type": "data", "value": 1}, reference(3)], [{"type": "data", "value": True}, reference(3)]]}]},
            {"type": "list", "value": [{"type": "set", "value": [{"type": "data", "value": None}, reference(4)]}]},
            {"type": "list", "value": [{"type": "map", "value": [[{"type": "data", "value": 2**53}, {"type": "data", "value": 1}]]}]},
            {"type": "list", "value": [{"type": "record", "value": {"a": {"type": "uuid", "value": "X"}, "b": reference(5)}}]},
        ]
        released = []
        for sequence, args in enumerate(refused, 1):
            peer._receive(self.invoke(sequence, args))
            while True:
                frame = await read()
                if frame["op"] == "release":
                    released.append((frame["reference"], frame["count"]))
                    continue
                break
            self.assertEqual(frame["op"], "throw", frame)
            self.assertEqual(frame["id"], f"main:{sequence}")
            self.assertEqual(frame["error"]["name"], "ValueError")
        self.assertEqual(sorted(released), [(1, 1), (2, 1), (3, 2), (4, 1), (5, 1)])
        self.assertEqual(peer.references(), {"exports": 0, "imports": 0})
        self.assertEqual(called, [])
        peer._receive(self.invoke(len(refused) + 1, {"type": "list", "value": [{"type": "date", "value": "2026-10-15"}]}))
        self.assertEqual(await read(), {"op": "return", "id": "main:6", "value": {"type": "data", "value": "ok"}})
        self.assertEqual(called, [[date(2026, 10, 15)]])
        self.assertIsNone(peer.closed_error)

    async def test_a_refused_result_fails_the_call_waiting_for_it(self):
        peer, read = self.session(lambda target, method, args: None)
        peer._deliver({"op": "return", "id": "py:1", "value": {"type": "instant", "value": "2026-10-15T03:22:15+02:00"}})
        with self.assertRaisesRegex(ValueError, "invalid instant"):
            peer.call("test", "m", [])
        await read()
        peer._deliver({"op": "return", "id": "py:2", "value": {"type": "instant", "value": "2026-10-15T03:22:15.9999999Z"}})
        self.assertEqual(peer.call("test", "m", []), datetime(2026, 10, 15, 3, 22, 15, 999999, tzinfo=timezone.utc))
        self.assertIsNone(peer.closed_error)

    async def test_a_tagged_value_of_the_wrong_shape_is_a_malformed_frame(self):
        for value in (
            {"type": "instant", "value": 5},
            {"type": "map", "value": [[{"type": "data", "value": 1}]]},
            {"type": "map", "value": {}},
            {"type": "set", "value": "x"},
        ):
            with self.subTest(value=value):
                peer, _read = self.session(lambda target, method, args: None)
                peer.receive(self.invoke(1, {"type": "list", "value": [value]}))
                self.assertIsNotNone(peer.closed_error)

    async def test_a_callback_in_a_map_value_crosses_by_reference(self):
        peer, _read = self.session(lambda target, method, args: None)
        wire = peer._encode({1: print}, [], False)
        self.assertEqual(wire["value"][0][1]["type"], "reference")
        decoded = peer._decode(
            {"type": "map", "value": [[{"type": "uuid", "value": str(UUID(int=1))}, {"type": "reference", "value": {"id": 9, "kind": "function", "home": False, "origin": []}}]]}
        )
        self.assertIsInstance(decoded[UUID(int=1)], RemoteFunction)


if __name__ == "__main__":
    unittest.main()

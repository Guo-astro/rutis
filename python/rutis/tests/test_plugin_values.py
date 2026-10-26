"""Plugin definitions as values (docs/design-plugin-values-2026-10-25.md):
a `define_plugin` result crosses as a `plugin` reference, only to a far end
that declared `plugins`; back home it is the same value, described by
`plugins.read` and loaded as a row's entry. The testing tool hands it over
as the runtime does (R10)."""

import asyncio
import json
import socket

import background

from rutis import define_plugin
from rutis.peer import Peer
from rutis.runner import FEATURES, Runtime
from rutis.testing import load


def part(ctx, config):
    ctx.provide("echo", Echo(config["n"]))


class Echo:
    def __init__(self, n):
        self.n = n

    def say(self):
        return f"echo {self.n}"


class SessionTests(background.IsolatedAsyncioTestCase):
    def session(self, capabilities):
        ours, theirs = socket.socketpair()
        theirs.settimeout(10)
        self.addCleanup(ours.close)
        self.addCleanup(theirs.close)
        peer = Peer(ours, lambda target, method, args: None)
        self.addCleanup(peer._channel.close)
        self.addCleanup(peer.close)
        peer._receive({"op": "hello", "version": 2, "capabilities": capabilities})
        lines = theirs.makefile("rb")
        self.addCleanup(lines.close)

        async def read():
            return json.loads(await asyncio.to_thread(lines.readline))

        return peer, read

    async def test_a_definition_crosses_as_a_plugin_reference(self):
        peer, _read = self.session(["plugins"])
        definition = define_plugin(part, inject=["tools"])
        wire = peer._encode([definition, definition], [], False, -1)
        first, second = wire["value"]
        self.assertEqual(first["type"], "reference")
        self.assertEqual(first["value"]["kind"], "plugin")
        self.assertFalse(first["value"]["home"])
        # The same value handed over twice is one reference: one definition.
        self.assertEqual(first, second)
        # Inside a record it is still a reference, not data.
        nested = peer._encode({"part": definition}, [], False)
        self.assertEqual(nested["type"], "record")
        self.assertEqual(nested["value"]["part"]["value"]["kind"], "plugin")

    async def test_a_far_end_without_plugins_fails_only_that_call(self):
        peer, read = self.session([])
        with self.assertRaisesRegex(ValueError, "did not declare the capability plugins"):
            peer.call("test", "register", [define_plugin(part)])
        self.assertEqual(peer._exports, {}, "nothing was granted")
        peer._deliver({"op": "return", "id": "node:2", "value": {"type": "data", "value": 7}})
        self.assertEqual(peer.call("test", "good", [1]), 7)
        self.assertEqual((await read())["id"], "node:2")
        self.assertIsNone(peer.closed_error)

    async def test_back_home_it_is_the_same_value(self):
        peer, _read = self.session(["plugins"])
        definition = define_plugin(part)
        sent = peer._encode(definition, [], False)
        back = dict(sent["value"], home=True)
        self.assertIs(peer._decode({"type": "reference", "value": back}), definition)
        # Calling it is not a thing: it is not a function or an object.
        with self.assertRaises(Exception):
            peer._export(back["id"], "function")

    async def test_a_plugin_reference_from_the_far_end_is_refused(self):
        peer, _read = self.session(["plugins"])
        with self.assertRaisesRegex(ValueError, "plugin definitions exported by Rust are not supported"):
            peer._decode_reference({"id": 1, "home": False, "kind": "plugin", "origin": []})


class RuntimeTests(background.TestCase):
    def test_the_runtime_declares_plugin_values(self):
        self.assertIn("plugin-values", FEATURES)

    def test_plugins_read_describes_a_definition(self):
        runtime = Runtime()
        definition = define_plugin(
            part,
            inject=["tools"],
            provides={"echo": Echo},
            config={"type": "object"},
            name="part",
        )
        self.assertEqual(
            runtime._control("plugins.read", [definition]),
            {
                "name": "part",
                "config": {"type": "object"},
                "inject": ["tools"],
                "provides": {"echo": {"say": "sync"}},
                "announces": True,
                "version": None,
            },
        )
        self.assertIsNone(runtime._control("plugins.read", [define_plugin(part)])["name"])
        with self.assertRaisesRegex(TypeError, "not a plugin definition"):
            runtime._control("plugins.read", [{"apply": part}])

    def test_rows_load_takes_a_definition_as_its_entry(self):
        runtime = Runtime()
        prepared = []

        def origin_part(ctx, config):
            prepared.append(config["n"])
            ctx.provide("echo", Echo(config["n"]))
            return lambda: prepared.append(f"{config['n']} down")

        definition = define_plugin(origin_part)

        async def run():
            # Two copies of one definition, each with its own config.
            await runtime._control("rows.load", ["a", definition, {"n": "A"}, [["echo", "A"]], [], {}, {"scope": [], "id": "origin"}])
            await runtime._control("rows.load", ["b", definition, {"n": "B"}, [["echo", "B"]], [], {}, {"scope": [], "id": "origin"}])
            seen = sorted(runtime.services)
            await runtime._control("rows.unload", ["a"])
            return seen

        seen = asyncio.run(run())
        self.assertEqual(prepared, ["A", "B", "A down"])
        self.assertEqual(len(seen), 2)
        self.assertEqual(runtime.rows["b"].module, definition)


class TestingToolTests(background.IsolatedAsyncioTestCase):
    async def test_a_definition_handed_to_a_double_can_be_loaded(self):
        received = []

        class Parts:
            def register(self, scope, definition):
                received.append(definition)

        def origin(ctx, config):
            ctx.use("parts").register("session", define_plugin(part, name="part"))

        async with load(define_plugin(origin, inject=["parts"]), services={"parts": Parts()}):
            pass
        self.assertEqual(len(received), 1)
        async with load(received[0], config={"n": "T"}) as copy:
            self.assertEqual(await copy.service("echo").say(), "echo T")


if __name__ == "__main__":
    import unittest

    unittest.main()

"""What a plugin author reads uses only what they wrote: service and method
names, arguments, rows; never this SDK's internals (`HostProxy`,
`UNDEFINED`, `RemoteFuture`), "Rust", or JSON library text. Each text is
locked here, with the code a program decides by (diagnostics design
2026-10-26 §4.3, §4.5; #677, #680). guarantee: sdk/error-texts"""

import asyncio
import json
import os
import socket
import tempfile
import traceback
import types
import sys
import unittest

import background
from support import LOCAL, hello

from rutis import define_plugin
from rutis.channel import SocketChannel
from rutis.peer import UNDEFINED, BindingError, LoadError, Peer, RemoteFunction, RemoteFuture, _Import, encode_error
from rutis.runner import HostProxy, Runtime
from rutis._unhandled import report_text, unhandled_error
from rutis.peer import AggregateError
from rutis.testing import load


class FakePeer:
    def __init__(self, result=None, error=None):
        self.result, self.error = result, error

    def call(self, target, method, args):
        if self.error is not None:
            raise self.error
        return self.result

    async def call_async(self, target, method, args):
        return self.call(target, method, args)


class Exported(unittest.TestCase):
    def test_load_error_is_exported(self):
        import rutis.advanced

        self.assertIs(rutis.advanced.LoadError, LoadError)
        self.assertIn("LoadError", rutis.advanced.__all__)

    def test_a_sync_wait_cycle_prints_its_message_alone(self):
        from rutis.advanced import SyncWaitCycle
        from rutis.peer import decode_error

        error = decode_error({"name": "SyncWaitCycle", "message": "a.b would wait on itself: x → y → x (synchronous calls)"})
        self.assertIsInstance(error, SyncWaitCycle)
        self.assertEqual(str(error), "a.b would wait on itself: x → y → x (synchronous calls)")


class HostProxyTexts(unittest.TestCase):
    def test_a_traceback_through_a_host_service_names_the_service_and_method(self):
        proxy = HostProxy(FakePeer(error=KeyError("region")), "weather\0tenant", {"today": "sync"})
        try:
            proxy.today("Oslo")
        except KeyError as error:
            printed = "".join(traceback.format_exception(error))
        self.assertIn("in weather.today", printed)
        self.assertNotIn("HostProxy", printed)
        self.assertNotIn("<locals>", printed)
        self.assertEqual(proxy.today.__qualname__, "weather.today")
        self.assertIn("weather.today", repr(proxy.today))
        self.assertNotIn("HostProxy", repr(proxy.today))
        self.assertEqual(repr(proxy), "<service weather>")

    def test_a_method_the_host_service_lacks_is_no_method(self):
        proxy = HostProxy(FakePeer(), "weather", {"today": "sync", "tomorrow": "async"})
        with self.assertRaises(AttributeError) as missing:
            proxy.tomorow
        self.assertEqual(str(missing.exception), "weather has no method tomorow (methods: today, tomorrow)")
        self.assertEqual(missing.exception.code, "no-method")
        self.assertEqual(
            missing.exception.data, {"service": "weather", "method": "tomorow", "methods": ["today", "tomorrow"]}
        )
        self.assertFalse(hasattr(proxy, "tomorow"))
        with self.assertRaises(AttributeError) as none:
            HostProxy(FakePeer(), "empty", {}).run
        self.assertEqual(str(none.exception), "empty has no method run (no methods)")

    def test_an_omitted_result_is_none(self):
        proxy = HostProxy(FakePeer(result=UNDEFINED), "svc", {"sync": "sync", "later": "async"})
        self.assertIsNone(proxy.sync())
        self.assertIsNone(asyncio.run(proxy.later()))
        self.assertEqual(repr(UNDEFINED), "None")

    def test_references_from_the_host_have_plain_reprs(self):
        record = _Import(1, "function", [])
        self.assertEqual(repr(RemoteFunction(None, record)), "<function from the host>")
        self.assertEqual(repr(RemoteFuture(None, record)), "<awaitable result from the host>")


class SessionTexts(background.IsolatedAsyncioTestCase):
    def session(self, dispatch=lambda target, method, args: None):
        ours, theirs = socket.socketpair()
        theirs.settimeout(10)
        self.addCleanup(ours.close)
        self.addCleanup(theirs.close)
        peer = Peer(SocketChannel(ours), dispatch, endpoint=LOCAL)
        self.addCleanup(peer._channel.close)
        self.addCleanup(peer.close)
        peer._receive(hello())
        lines = theirs.makefile("rb")
        self.addCleanup(lines.close)

        async def read():
            return json.loads(await asyncio.to_thread(lines.readline))

        return peer, theirs, read

    async def test_calls_waiting_when_the_host_goes_fail_with_peer_gone(self):
        peer, theirs, read = self.session()
        peer.start()
        self.assertEqual((await read())["op"], "hello")
        future = peer.request("host:weather", "today", [])
        await read()
        theirs.shutdown(socket.SHUT_RDWR)
        with self.assertRaises(ConnectionError) as gone:
            await asyncio.wait_for(future, 10)
        self.assertEqual(str(gone.exception), "the host disconnected")
        self.assertEqual(gone.exception.code, "peer-gone")
        self.assertEqual(gone.exception.data, {"reason": "disconnected"})
        # Later calls fail the same way.
        with self.assertRaisesRegex(ConnectionError, "^the host disconnected$"):
            peer.request("host:weather", "today", [])

    async def test_a_number_that_is_not_finite_cannot_be_sent_said_plainly(self):
        peer, _theirs, read = self.session()
        for value, shown in ((float("nan"), "NaN"), (float("inf"), "Infinity"), ([1, {"x": float("-inf")}], "-Infinity")):
            with self.subTest(shown):
                with self.assertRaises(ValueError) as refused:
                    peer.call("host:weather\0t1", "tomorrow", ["Oslo", value])
                self.assertEqual(
                    str(refused.exception), f"weather.tomorrow: argument 2 cannot be sent: {shown} cannot cross processes"
                )
                self.assertEqual(refused.exception.code, "bad-value")
                self.assertEqual(refused.exception.data, {"part": "argument", "argument": 2})
        self.assertIsNone(peer.closed_error)

    async def test_a_result_that_is_not_finite_is_answered_with_why(self):
        peer, _theirs, read = self.session(lambda target, method, args: {"t": float("nan")})
        peer._receive(
            {"op": "invoke", "id": "main:1", "path": [], "target": "weather", "method": "today",
             "args": {"type": "data", "value": []}}
        )
        reply = await read()
        self.assertEqual(reply["op"], "throw", reply)
        self.assertEqual(reply["error"]["message"], "weather.today: the result cannot be sent: NaN cannot cross processes")
        root = reply["error"]["graph"]["nodes"][reply["error"]["graph"]["root"]["value"]]
        self.assertEqual(root["code"], "bad-value")
        self.assertIsNone(peer.closed_error)


def module(name, apply, inject=()):
    plugin = types.ModuleType(name)
    plugin.apply = apply
    plugin.inject = list(inject)
    sys.modules[name] = plugin
    return name


class Weather:
    def today(self, city):
        return f"sunny in {city}"

    def _private(self):
        return "hidden"


class ProvidedServiceCalls(background.TestCase):
    """A call of a service a Python plugin provides, with a method it does
    not have or arguments it cannot take, is refused before it runs."""

    def runtime(self):
        runtime = Runtime()
        name = module("plain_texts_weather", lambda ctx, config: ctx.provide("weather", Weather()))
        asyncio.run(runtime.load("w", name, {}, {"weather": ["today"]}, id="weather-row"))
        return runtime

    def refused(self, method, args):
        with self.assertRaises(BindingError) as refused:
            self.runtime().dispatch("weather", method, args)
        return refused.exception

    def test_no_method(self):
        error = self.refused("tomorow", [])
        self.assertEqual(error.message, "weather has no method tomorow (methods: today)")
        self.assertEqual((error.code, error.data), ("no-method", {"service": "weather", "method": "tomorow", "methods": ["today"]}))

    def test_too_many_arguments(self):
        error = self.refused("today", ["Oslo", "extra"])
        self.assertEqual(error.message, "weather.today takes 1 argument, got 2")
        self.assertEqual(
            (error.code, error.data),
            ("bad-argument", {"service": "weather", "method": "today", "expected": 1, "received": 2}),
        )

    def test_a_missing_argument(self):
        error = self.refused("today", [])
        self.assertEqual(error.message, "weather.today: argument 1 is missing")
        self.assertEqual(
            (error.code, error.data),
            ("bad-argument", {"service": "weather", "method": "today", "argument": 1, "expected": None}),
        )

    def test_a_good_call_runs(self):
        self.assertEqual(self.runtime().dispatch("weather", "today", ["Oslo"]), "sunny in Oslo")

    def test_a_service_provided_twice_names_the_row(self):
        runtime = self.runtime()
        name = module("plain_texts_again", lambda ctx, config: ctx.provide("weather", Weather()))
        with self.assertRaises(ValueError) as twice:
            asyncio.run(runtime.load("x", name, {}, None, id="again"))
        self.assertEqual(str(twice.exception), "weather is already provided by weather-row")


class TestingToolTexts(background.TestCase):
    """`rutis.testing` says what the runtime says."""

    def run_with(self, check):
        plugin = define_plugin(
            lambda ctx, config: ctx.provide("weather", Weather()),
            provides={"weather": {"today": "sync"}},
        )

        async def go():
            async with load(plugin) as t:
                check(t.service("weather"))

        asyncio.run(go())

    def test_no_method(self):
        def check(weather):
            with self.assertRaises(AttributeError) as missing:
                weather.tomorow
            self.assertEqual(str(missing.exception), "weather has no method tomorow (methods: today)")
            self.assertEqual(missing.exception.code, "no-method")

        self.run_with(check)

    def test_bad_arguments(self):
        def check(weather):
            with self.assertRaises(BindingError) as many:
                weather.today("Oslo", "extra")
            self.assertEqual(many.exception.message, "weather.today takes 1 argument, got 2")
            self.assertEqual(many.exception.code, "bad-argument")
            with self.assertRaises(BindingError) as missing:
                weather.today()
            self.assertEqual(missing.exception.message, "weather.today: argument 1 is missing")

        self.run_with(check)

class LoadFailures(background.TestCase):
    """A plugin module whose import raises fails with a LoadError naming
    the file that raised (a module the entry imports, say), what and where,
    as Node and Bun say it (diagnostics design §8.1, row 5); its stack is
    the plugin's frames only."""

    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.dir = os.path.realpath(directory.name)
        files = {
            "lf_entry.py": "import lf_util\n\ndef apply(ctx, config):\n    pass\n",
            "lf_util.py": "x = 1\n\nraise KeyError('region')\n",
            "lf_syntax.py": "import lf_bad\n\ndef apply(ctx, config):\n    pass\n",
            "lf_bad.py": "x = 1\ndef broken(:\n    pass\n",
            "lf_json.py": "import lf_parse\n\ndef apply(ctx, config):\n    pass\n",
            "lf_parse.py": "import json\n\nSETTINGS = json.loads('{bad')\n",
        }
        for name, text in files.items():
            with open(os.path.join(self.dir, name), "w") as file:
                file.write(text)
        sys.path.insert(0, self.dir)
        self.addCleanup(sys.path.remove, self.dir)
        cwd = os.getcwd()
        os.chdir(self.dir)
        self.addCleanup(os.chdir, cwd)
        for name in ("lf_entry", "lf_util", "lf_syntax", "lf_bad", "lf_json", "lf_parse"):
            self.addCleanup(sys.modules.pop, name, None)

    def test_the_submodule_that_raised_is_named(self):
        with self.assertRaises(LoadError) as failed:
            Runtime().describe("lf_entry")
        error = failed.exception
        self.assertEqual(str(error), "cannot load lf_util.py: KeyError: 'region' (lf_util.py:3)")
        # The plugin's frames, not the runtime's or the import machinery's.
        self.assertTrue(error.stack.startswith("LoadError: cannot load lf_util.py: "), error.stack)
        self.assertIn("lf_entry.py\", line 1, in <module>", error.stack)
        self.assertIn("lf_util.py\", line 3, in <module>", error.stack)
        self.assertNotIn("runner.py", error.stack)
        self.assertNotIn("importlib", error.stack)
        # It crosses as a LoadError with that stack.
        node = encode_error(error)["graph"]["nodes"][0]
        self.assertEqual((node["name"], node["message"], node["stack"]), ("LoadError", str(error), error.stack))

    def test_a_syntax_error_says_its_file_and_line(self):
        with self.assertRaises(LoadError) as failed:
            Runtime().describe("lf_syntax")
        self.assertEqual(str(failed.exception), "cannot load lf_bad.py: SyntaxError: invalid syntax (lf_bad.py:2)")

    def test_a_standard_library_call_that_raises_names_the_plugin_file(self):
        # json raises inside its own decoder: the file named is the
        # plugin's, which called it, not json/decoder.py.
        with self.assertRaises(LoadError) as failed:
            Runtime().describe("lf_json")
        self.assertEqual(
            str(failed.exception),
            "cannot load lf_parse.py: JSONDecodeError: Expecting property name enclosed in double quotes: "
            "line 1 column 2 (char 1) (lf_parse.py:3)",
        )
        self.assertNotIn("decoder.py", failed.exception.stack)

    def test_loading_a_row_fails_the_same_way(self):
        with self.assertRaisesRegex(LoadError, "^cannot load lf_util.py: KeyError: 'region' \\(lf_util.py:3\\)$"):
            asyncio.run(Runtime().load("w", "lf_entry", {}, None, id="weather"))

    def test_a_module_that_does_not_exist_is_said_as_it_is(self):
        with self.assertRaises(ModuleNotFoundError) as missing:
            Runtime().describe("lf_nowhere")
        self.assertEqual(str(missing.exception), "No module named 'lf_nowhere'")


class UnhandledTexts(unittest.TestCase):
    """guarantee: loader/unhandled-errors, sdk/error-texts"""

    @staticmethod
    def raised(error):
        try:
            raise error
        except BaseException as caught:  # noqa: BLE001 - the test's own
            return caught

    def test_an_error_left_unhandled_while_loading_crosses_with_its_cause_and_its_errors(self):
        inner = self.raised(KeyError("region"))
        try:
            try:
                raise inner
            except KeyError as cause:
                raise AggregateError([ValueError("one"), TypeError("two")], "two failed") from cause
        except AggregateError as caught:
            aggregate = caught
        failure = unhandled_error("unhandled exception", aggregate)
        self.assertEqual(str(failure), "unhandled exception: AggregateError: two failed")
        graph = encode_error(failure)["graph"]
        nodes = graph["nodes"]
        root = nodes[graph["root"]["value"]]
        cause = nodes[root["cause"]["value"]]
        self.assertEqual(cause["name"], "AggregateError")
        self.assertEqual([nodes[ref["value"]]["message"] for ref in cause["errors"]], ["one", "two"])
        self.assertEqual(nodes[cause["cause"]["value"]]["name"], "KeyError")
        # The failure's stack holds the error's frames once, not its chain again.
        self.assertNotIn("direct cause", root["stack"])
        self.assertEqual(root["stack"].count("raise AggregateError"), 1)

    def test_an_error_from_no_row_lists_only_the_plugins_frames(self):
        error = self.raised(ValueError("boom"))
        text = report_text(error)
        self.assertTrue(text.startswith("Traceback (most recent call last):"), text)
        self.assertIn("test_plain_texts.py", text)
        self.assertTrue(text.endswith("ValueError: boom"), text)
        # The chain stays, each part with the plugin's frames only.
        try:
            try:
                raise KeyError("region")
            except KeyError as cause:
                raise ValueError("no forecast") from cause
        except ValueError as caught:
            chained = caught
        text = report_text(chained)
        self.assertIn("KeyError: 'region'", text)
        self.assertIn("direct cause", text)
        self.assertTrue(text.endswith("ValueError: no forecast"), text)
        # None of its frames is the plugin's: the whole traceback.
        import json as library

        try:
            library.loads("{bad")
        except ValueError as caught:
            error = caught
        error.__traceback__ = error.__traceback__.tb_next
        self.assertIn("decoder.py", report_text(error))


if __name__ == "__main__":
    unittest.main()

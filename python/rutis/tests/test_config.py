"""A row's config against its plugin's schema (#669, config schema design
§3–§6): every case of the shared table specs/config-schema/cases.json,
word for word, through the SDK's check, the testing tool and the runtime,
which share one piece of code (`rutis.config.configured`)."""

import asyncio
import contextlib
import io
import json
import os
import sys
import types
import unittest

import background

from rutis import BindingError, define_plugin
from rutis.config import check, config_lines
from rutis.runner import Runtime
from rutis.testing import load

CASES = json.load(
    open(os.path.join(os.path.dirname(__file__), "..", "..", "..", "specs", "config-schema", "cases.json"), encoding="utf-8")
)["cases"]


def strictly_equal(a, b):
    """JSON equality that also tells integers from fractions (and booleans
    from numbers)."""
    if type(a) is not type(b):
        return False
    if isinstance(a, dict):
        return a.keys() == b.keys() and all(strictly_equal(a[key], b[key]) for key in a)
    if isinstance(a, list):
        return len(a) == len(b) and all(strictly_equal(x, y) for x, y in zip(a, b))
    return a == b


def touched(value):
    """`value` changed in place everywhere: a key added to every object, an
    item to every array."""
    if isinstance(value, dict):
        for child in list(value.values()):
            touched(child)
        value["__touched__"] = True
    elif isinstance(value, list):
        for child in value:
            touched(child)
        value.append("touched")


def given(case):
    """The config a case gives (no `config`: none)."""
    return case.get("config")


class Cases(unittest.TestCase):
    """Each case through `rutis.config.check`."""

    def test_every_case(self):
        for case in CASES:
            with self.subTest(case["name"]):
                checked = check(case["schema"], given(case))
                self.assertEqual([f"config: {w.message}" for w in checked.warnings], case.get("warnings", []))
                if "errors" in case["then"]:
                    self.assertEqual(config_lines(checked.errors), case["then"]["errors"])
                else:
                    self.assertEqual(config_lines(checked.errors), [])
                    self.assertTrue(strictly_equal(checked.config, case["then"]["config"]), checked.config)

    def test_defaults_are_copies(self):
        # Design §4, case 21: a plugin changing what it got changes nothing
        # of the next load.
        for case in CASES:
            if "config" not in case["then"]:
                continue
            with self.subTest(case["name"]):
                first = check(case["schema"], given(case))
                touched(first.config)
                second = check(case["schema"], given(case))
                self.assertTrue(strictly_equal(second.config, case["then"]["config"]), second.config)

    def test_the_given_config_is_not_changed(self):
        config = {"home": {}}
        schema = {"type": "object", "properties": {"home": {"type": "object", "properties": {"lat": {"default": 1}}}}}
        self.assertEqual(check(schema, config).config, {"home": {"lat": 1}})
        self.assertEqual(config, {"home": {}})

    def test_a_bad_config_is_a_binding_error_with_its_issues(self):
        schema = {"type": "object", "properties": {"units": {"enum": ["metric", "imperial"]}, "days": {"type": "integer"}}}
        checked = check(schema, {"units": "kelvin", "days": 1.5})
        from rutis.config import config_error

        error = config_error(checked.errors)
        self.assertIsInstance(error, BindingError)
        self.assertEqual(
            str(error),
            'config: days should be an integer, got 1.5\nconfig: units should be "metric" or "imperial", got "kelvin"',
        )
        self.assertEqual(error.code, "bad-config")
        self.assertEqual(
            error.data,
            {
                "issues": [
                    {"path": ["days"], "message": "days should be an integer, got 1.5"},
                    {"path": ["units"], "message": 'units should be "metric" or "imperial", got "kelvin"'},
                ]
            },
        )


class Branches(unittest.TestCase):
    def test_true_branches_are_not_remembered_by_identity(self):
        # Every JSON `true` is the same Python object: a second `true`
        # branch must not give back the value as it was before another
        # branch normalised it.
        schema = {
            "type": "object",
            "allOf": [
                {"anyOf": [True]},
                {"anyOf": [{"properties": {"n": {"type": "integer"}}}]},
                {"anyOf": [True]},
            ],
        }
        self.assertTrue(strictly_equal(check(schema, {"n": 1.0}).config, {"n": 1}))

    def test_a_key_that_is_not_a_string_is_refused_without_a_crash(self):
        checked = check({"type": "object", "properties": {"city": {}}, "additionalProperties": False}, {1: "x"})
        self.assertEqual(config_lines(checked.errors), ["config: config[1] is not a config key"])


class TestingTool(unittest.TestCase):
    """Each case through `rutis.testing.load`: what `apply` gets, the error
    `load` raises, the warnings on standard error."""

    def run_case(self, case):
        got = {}

        def apply(ctx, config):
            got["config"] = config

        plugin = define_plugin(apply, config=case["schema"])
        stderr = io.StringIO()

        async def go():
            async with load(plugin, config=given(case), row="weather"):
                pass

        with contextlib.redirect_stderr(stderr):
            try:
                asyncio.run(go())
            except BindingError as error:
                got["error"] = error
        return got, stderr.getvalue()

    def test_every_case(self):
        for case in CASES:
            with self.subTest(case["name"]):
                got, stderr = self.run_case(case)
                warned = [line.removeprefix("weather: ") for line in stderr.splitlines()]
                if "errors" in case["then"]:
                    self.assertNotIn("config", got, "apply is not called")
                    self.assertEqual(got["error"].code, "bad-config")
                    self.assertEqual(str(got["error"]).split("\n"), case["then"]["errors"])
                    # The warnings come before the error, as from the loader.
                    self.assertEqual(warned, case.get("warnings", []))
                else:
                    self.assertNotIn("error", got)
                    self.assertTrue(strictly_equal(got["config"], case["then"]["config"]), got["config"])
                    self.assertEqual(warned, case.get("warnings", []))

    def test_a_plugin_without_a_schema_gets_what_it_is_given(self):
        got = []
        plugin = define_plugin(lambda ctx, config: got.append(config))

        async def go():
            async with load(plugin):
                pass
            async with load(plugin, config="Oslo"):
                pass

        asyncio.run(go())
        self.assertEqual(got, [{}, "Oslo"])

    def test_a_config_crosses_before_it_is_checked(self):
        # As it reaches a runtime: a tuple is an array there.
        got = []
        plugin = define_plugin(lambda ctx, config: got.append(config), config={"type": "object", "properties": {"xs": {"type": "array"}}})

        async def go():
            async with load(plugin, config={"xs": (1, 2)}):
                pass

        asyncio.run(go())
        self.assertEqual(got, [{"xs": [1, 2]}])

    def test_an_extra_key_is_warned_about_once(self):
        plugin = define_plugin(lambda ctx, config: None, config={"type": "object", "properties": {"city": {"type": "string"}}})
        stderr = io.StringIO()

        async def go():
            async with load(plugin, config={"cty": "Oslo"}, row="weather") as t:
                await t.update({"cty": "Oslo"})

        with contextlib.redirect_stderr(stderr):
            asyncio.run(go())
        self.assertEqual(stderr.getvalue(), "weather: config: cty is not a config key (did you mean city?)\n")

    def test_update_checks_the_new_config(self):
        schema = {"type": "object", "properties": {"days": {"type": "integer", "default": 3}}}
        got = []
        plugin = define_plugin(lambda ctx, config: got.append(config), config=schema)

        async def go():
            async with load(plugin) as t:
                await t.update({"days": 5.0})
                with self.assertRaises(BindingError) as refused:
                    await t.update({"days": "five"})
                self.assertEqual(str(refused.exception), 'config: days should be an integer, got "five"')

        asyncio.run(go())
        self.assertTrue(strictly_equal(got, [{"days": 3}, {"days": 5}]), got)


def module(name, schema, got):
    plugin = types.ModuleType(name)
    plugin.apply = lambda ctx, config: got.append(config)
    plugin.Config = schema
    sys.modules[name] = plugin
    return name


class RuntimeRows(background.TestCase):
    """Each case through the runtime's load of a module row: the same code
    as the testing tool, no warnings (the host's loader warns)."""

    def test_every_case(self):
        for index, case in enumerate(CASES):
            with self.subTest(case["name"]):
                got = []
                name = module(f"config_case_{index}", case["schema"], got)
                runtime = Runtime()
                stderr = io.StringIO()
                with contextlib.redirect_stderr(stderr):
                    try:
                        asyncio.run(runtime.load("w", name, given(case), {}, id="weather"))
                        error = None
                    except BindingError as refused:
                        error = refused
                self.assertEqual(stderr.getvalue(), "")
                if "errors" in case["then"]:
                    self.assertEqual(got, [], "apply is not called")
                    self.assertIsNotNone(error)
                    self.assertEqual(error.code, "bad-config")
                    self.assertEqual(str(error).split("\n"), case["then"]["errors"])
                else:
                    self.assertIsNone(error)
                    self.assertTrue(strictly_equal(got[0], case["then"]["config"]), got)

    def test_a_module_without_a_schema_gets_what_it_is_given(self):
        got = []
        plugin = types.ModuleType("config_no_schema")
        plugin.apply = lambda ctx, config: got.append(config)
        sys.modules["config_no_schema"] = plugin
        asyncio.run(Runtime().load("w", "config_no_schema", "Oslo", {}, id="weather"))
        self.assertEqual(got, ["Oslo"])

    def test_a_bad_config_crosses_with_its_code_and_issues(self):
        from rutis.peer import encode_error

        got = []
        name = module("config_crosses", {"type": "object", "required": ["city"]}, got)
        with self.assertRaises(BindingError) as refused:
            asyncio.run(Runtime().load("w", name, None, {}, id="weather"))
        wire = encode_error(refused.exception)
        root = wire["graph"]["nodes"][wire["graph"]["root"]["value"]]
        self.assertEqual((wire["name"], wire["message"]), ("BindingError", "config: city is required"))
        self.assertEqual(root["code"], "bad-config")
        from rutis.peer import decode_error

        received = decode_error(wire)
        self.assertEqual((received.code, received.data), ("bad-config", {"issues": [{"path": ["city"], "message": "city is required"}]}))

    def test_a_definition_entry_is_checked_as_a_module_row(self):
        got = []
        definition = define_plugin(lambda ctx, config: got.append(config), config={"type": "object", "properties": {"days": {"type": "integer", "default": 3}}})
        plugin = types.ModuleType("config_definition")
        plugin.plugin = definition
        sys.modules["config_definition"] = plugin
        asyncio.run(Runtime().load("w", "config_definition", None, {}, id="weather"))
        with self.assertRaises(BindingError) as refused:
            asyncio.run(Runtime().load("w", "config_definition", {"days": "three"}, {}, id="weather"))
        self.assertEqual(str(refused.exception), 'config: days should be an integer, got "three"')
        self.assertEqual(got, [{"days": 3}])


if __name__ == "__main__":
    unittest.main()

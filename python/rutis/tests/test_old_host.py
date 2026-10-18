"""A 0.12 runtime under a host older than 0.12 (its mount has no
`announce`): a plugin providing a service it does not declare is told, once
per name, that the host does not take it."""

import asyncio
import contextlib
import io
import sys
import types

import background

from rutis.runner import Runtime


def module(name: str, apply, provides=None) -> str:
    plugin = types.ModuleType(name)
    plugin.apply = apply
    if provides is not None:
        plugin.provides = provides
    sys.modules[name] = plugin
    return name


class Service:
    def get(self):
        return 1


def provide(ctx, config):
    ctx.provide("declared", Service())
    ctx.provide("weather", Service())
    ctx.provide("weather#1", Service())


MESSAGE = (
    "rutis: service weather is not declared in provides, and this host (older than 0.12) "
    "does not take undeclared services; declare it or upgrade rutis-host\n"
)


class OldHost(background.TestCase):
    def loaded_twice(self, mount: dict) -> str:
        runtime = Runtime()
        runtime._control("mount", mount)
        entry = module("old_host_provider", provide, {"declared": {"get": "sync"}})
        exports = {"declared": {"get": "sync"}}

        async def run():
            await runtime.load("a", entry, {}, exports)
            await runtime.unload("a")
            await runtime.load("b", entry, {}, exports)
            await runtime.unload("b")

        written = io.StringIO()
        with contextlib.redirect_stderr(written):
            asyncio.run(run())
        return written.getvalue()

    def test_an_undeclared_service_is_told_once_per_name(self):
        self.assertEqual(self.loaded_twice({}), MESSAGE)

    def test_a_host_taking_undeclared_services_tells_nothing(self):
        self.assertEqual(self.loaded_twice({"announce": True}), "")

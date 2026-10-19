"""The `conformance-events` plugin for emitter rows labelled `e`: the same
plugin as conformance_events.py, declaring only its own service
`emitter-e` (crates/rutis-bridge/src/runtime/testing/events.expected.json)."""

from conformance_events import EMITTER, apply, inject
from rutis import define_plugin

provides = {"emitter-e": EMITTER}

plugin = define_plugin(apply, inject=inject, provides=provides, name="conformance-events")

__all__ = ["apply", "inject", "plugin", "provides"]

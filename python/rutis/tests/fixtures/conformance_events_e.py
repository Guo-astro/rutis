"""The `conformance-events` plugin for emitter rows labelled `e`: the same
plugin as conformance_events.py, declaring only its own service
`emitter-e` (crates/rutis-bridge/src/runtime/testing/events.expected.json)."""

from conformance_events import EMITTER, apply, inject

provides = {"emitter-e": EMITTER}

__all__ = ["apply", "inject", "provides"]

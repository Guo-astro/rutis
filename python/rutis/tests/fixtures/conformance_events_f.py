"""The `conformance-events` plugin for emitter rows labelled `f`: the same
plugin as conformance_events.py, declaring only its own service
`emitter-f` (crates/rutis-bridge/src/runtime/testing/events.expected.json)."""

from conformance_events import EMITTER, apply, inject

provides = {"emitter-f": EMITTER}

__all__ = ["apply", "inject", "provides"]

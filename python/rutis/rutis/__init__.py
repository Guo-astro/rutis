"""rutis for Python: write plugins (`define_plugin`, `rutis.testing`), and the
runtime that runs them (`python -m rutis <channel> <project>`, which a rutis
host starts).

See `rutis.plugin` for how a plugin is written. Values beyond JSON cross by
value (#308): datetime, date, time, bytes, set, Decimal, UUID, a dict with
keys that are not strings, and `BigInt` and `Map` from here.
"""

from ._values import BigInt, Map
from .peer import AggregateError, RemoteError, SyncWaitCycle
from .plugin import PLUGIN_API, Context, Plugin, define_plugin

__all__ = [
    "PLUGIN_API",
    "Context",
    "Plugin",
    "AggregateError",
    "BigInt",
    "Map",
    "RemoteError",
    "SyncWaitCycle",
    "define_plugin",
]

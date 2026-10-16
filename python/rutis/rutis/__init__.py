"""rutis for Python: write plugins (`define_plugin`, `rutis.testing`), and the
runtime that runs them (`python -m rutis <channel> <project>`, which a rutis
host starts).

See `rutis.plugin` for how a plugin is written. Plain Python values cross
by value, beyond JSON too (#308): datetime, date, time, bytes, set,
Decimal, UUID, a dict with keys that are not strings, an int of any size.
"""

from .peer import AggregateError, RemoteError, SyncWaitCycle
from .plugin import PLUGIN_API, Context, Plugin, define_plugin

__all__ = [
    "PLUGIN_API",
    "Context",
    "Plugin",
    "AggregateError",
    "RemoteError",
    "SyncWaitCycle",
    "define_plugin",
]

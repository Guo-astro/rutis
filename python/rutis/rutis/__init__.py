"""rutis for Python: write plugins (`define_plugin`, `rutis.testing`), and the
runtime that runs them (`python -m rutis <channel> <project>`, which a rutis
host starts).

See `rutis.plugin` for how a plugin is written. Plain Python values cross
by value, beyond JSON too (#308): datetime, date, time, bytes, set,
Decimal, UUID, a dict with keys that are not strings, an int of any size.

Events (rutis 0.13, #464) are one bus across every plugin, runtime and
linked node: `ctx.on` listens, `ctx.emit` / `parallel` / `serial` /
`waterfall` / `bail` emit, `ctx.filter` narrows who receives; see
`rutis.events` for the rules.
"""

from .peer import BindingError, RemoteError
from .plugin import Context, Plugin, define_plugin

__all__ = [
    "define_plugin",
    "Plugin",
    "Context",
    "RemoteError",
    "BindingError",
]

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

import warnings as _warnings

from . import advanced as _advanced
from .peer import BindingError, RemoteError
from .plugin import Context, Plugin, define_plugin

# Moved to `rutis.advanced` in 0.19 (#692). The old names still work and stay
# in `__all__` (so `from rutis import *` is unchanged); the first use warns
# with the new place. They are removed when the plugin API version changes.
_MOVED = frozenset(_advanced.__all__)


def __getattr__(name: str):
    if name not in _MOVED:
        raise AttributeError(f"module 'rutis' has no attribute {name!r}")
    _warnings.warn(
        f"{name} moved in 0.19: import it from rutis.advanced. "
        "The old name is removed when the plugin API version changes.",
        DeprecationWarning,
        stacklevel=2,
    )
    # Kept here: `from rutis import X` looks the name up twice.
    value = globals()[name] = getattr(_advanced, name)
    return value


def __dir__():
    return sorted(set(globals()) | _MOVED)


__all__ = [
    "define_plugin",
    "Plugin",
    "Context",
    "RemoteError",
    "BindingError",
    # Moved to rutis.advanced in 0.19, kept until the plugin API version changes.
    "PLUGIN_API",
    "AggregateError",
    "AsyncListener",
    "LoadError",
    "ReservedEvent",
    "SyncWaitCycle",
    "WaterfallNext",
]

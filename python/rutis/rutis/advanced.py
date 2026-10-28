"""What a plugin author rarely writes (#692): the plugin API version, and the
errors that only one way of emitting raises or that a runtime raises on its
own. `define_plugin`, `Plugin`, `Context`, `RemoteError` and `BindingError`
are in `rutis`.

    from rutis.advanced import AggregateError

    async def apply(ctx, config):
        try:
            await ctx.parallel("cache/flush")
        except AggregateError as error:
            ...
"""

from .peer import AggregateError, AsyncListener, LoadError, ReservedEvent, SyncWaitCycle, WaterfallNext
from .plugin import PLUGIN_API

__all__ = [
    "PLUGIN_API",
    "AggregateError",
    "AsyncListener",
    "LoadError",
    "ReservedEvent",
    "SyncWaitCycle",
    "WaterfallNext",
]

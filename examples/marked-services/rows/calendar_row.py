"""A Python row using the same Rust `Calendar` by name. Its methods and
types come from rows/services.py, which `rutis-host types --python` writes
from the Rust definitions; pyright checks the calls against them."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from rutis import Context, RemoteError

if TYPE_CHECKING:
    from services import Calendar

inject = ["calendar", "probe"]


async def apply(ctx: Context, config: Any) -> None:
    calendar: Calendar = ctx.use("calendar")
    probe = ctx.use("probe")
    id = await calendar.add({"title": "Planning", "startsAt": 2**60})
    entry = await calendar.get(id)
    probe.record(f"py: {entry['title']} {entry['startsAt'] == 2**60}")
    try:
        await calendar.get(99)
    except RemoteError as error:
        probe.record(f"py: {error.name} {error.code} {error.data}")

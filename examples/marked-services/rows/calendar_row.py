"""A Python row using the same Rust `Calendar` by name."""

from rutis import RemoteError

inject = ["calendar", "probe"]


async def apply(ctx, config):
    calendar = ctx.use("calendar")
    probe = ctx.use("probe")
    id = await calendar.add({"title": "Planning", "startsAt": 2**60})
    entry = await calendar.get(id)
    probe.record(f"py: {entry['title']} {entry['startsAt'] == 2**60}")
    try:
        await calendar.get(99)
    except RemoteError as error:
        probe.record(f"py: {error.name} {error.code} {error.data}")

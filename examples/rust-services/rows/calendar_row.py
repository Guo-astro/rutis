"""A Python row using the same Rust `Calendar`: one dispatch serves every
language."""

from rutis import RemoteError

inject = ["calendar", "probe"]


async def apply(ctx, config):
    calendar = ctx.use("calendar")
    probe = ctx.use("probe")
    added = await calendar.add({"title": "Planning", "startsAt": 2**60, "kind": "all_day"})
    got = await calendar.get(added["id"])
    probe.record(f"py got {got['title']} {got['kind']} {got['startsAt'] == 2**60}")
    try:
        await calendar.get(99999)
    except RemoteError as error:
        probe.record(f"py missing {error.name} {error.code} {error.message} {error.data}")

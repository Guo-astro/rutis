# Generated from the service declarations; do not edit.

from __future__ import annotations

import typing

import rutis

#: What a method's `Err(e)` raises: named after e's type, `code` its variant,
#: `message` its Display, `data` its serde value.
RustError = rutis.RemoteError


class Entry(typing.TypedDict):
    """A calendar entry."""

    title: str
    startsAt: int
    """Milliseconds since the Unix epoch."""


class _CalendarErrorNotFoundContent(typing.TypedDict):
    id: int


class _CalendarErrorNotFound(typing.TypedDict):
    NotFound: _CalendarErrorNotFoundContent


CalendarError: typing.TypeAlias = "_CalendarErrorNotFound"


#: How often a planned entry repeats.
Repeat: typing.TypeAlias = 'typing.Literal["never", "daily", "weekly"]'


class _ReminderOptional(typing.TypedDict, total=False):
    sound: bool
    """Off when left out."""
    note: str | None


class Reminder(_ReminderOptional):
    """A reminder before a planned entry."""

    minutesBefore: int


class Calendar(typing.Protocol):
    """The application's calendar."""

    async def add(self, entry: Entry) -> int:
        """Add an entry; its id."""
        ...

    async def get(self, id: int) -> Entry:
        """The entry `id`.

        Raises CalendarErrorThrown.
        """
        ...

    async def each(self, visit: typing.Callable[[Entry], None | typing.Awaitable[None]]) -> int:
        """Call `visit` with each entry, in order; how many there were.

        Raises ErrorThrown.
        """
        ...

    async def plan(self, title: str, all_day: bool, hours: float, repeat: Repeat, reminders: list[Reminder], window: tuple[int, int] | None, labels: dict[str, str]) -> int:
        """Plan an entry; its number. Its parameters take one of each kind of
        value a service is given, so that the generated declarations show
        each.
        """
        ...

    async def addSource(self, name: str, source: rutis.Plugin) -> int:
        """Add a plugin that brings entries from elsewhere, by name; how many
        there are.
        """
        ...


#: What a method returning `Err(CalendarError)` raises: a RustError named "CalendarError", its
#: `code` one of CalendarErrorThrownCode.
CalendarErrorThrown = RustError
CalendarErrorThrownCode: typing.TypeAlias = 'typing.Literal["NotFound"]'


#: What a method returning `Err(Error)` raises: a RustError named "Error", its
#: `code` one of ErrorThrownCode.
ErrorThrown = RustError
ErrorThrownCode: typing.TypeAlias = "str"

"""The tagged values beyond JSON data (#308, #355, #360): points in time,
local dates and times, binary data, maps, sets, big integers, decimals and
UUIDs. Each crosses a session under its own tag, as text in the formats
below, and only to a far end that declared the capability `value-<tag>`.

| tag | Python | text on the wire |
| --- | --- | --- |
| `instant` | `datetime` with a time zone (received in UTC) | `2026-10-15T03:22:15.123456Z` |
| `datetime` | `datetime` without a time zone | `2026-10-15T14:30:05.25` |
| `date` | `date` | `2026-10-15` |
| `time` | `time` without a time zone | `14:30:05.25` |
| `bytes` | `bytes` (`bytearray`, `memoryview` sent too) | base64 (RFC 4648 §4, padded) |
| `map` | a dict with a key that is not a string; a received map (`Map`) | `[[key, value], …]` |
| `set` | `set` (`frozenset` sent too) | `[element, …]` |
| `bigint` | an int beyond ±(2^53−1); a received bigint (`BigInt`) | decimal integer, at most 4300 digits |
| `decimal` | `Decimal` | decimal, no exponent, trailing zeros kept |
| `uuid` | `UUID` | lowercase `8-4-4-4-12` hex |

Plugins write plain Python (design V0): `BigInt` and `Map` are internal,
what a received bigint or map arrives as, so that passed on it keeps its
tag (a JS `5n` comes back `5n`, a JS `Map` with string keys a `Map`).

Python keeps microseconds: an instant, local date and time or time of day
with more digits is truncated toward negative infinity when received
(design V7). Design: docs/design-value-types-2026-10-15.md; the normative
wire text is docs/design-protocol-plugins-2026-09-25.md §6.1; the Rust
reference is crates/rutis-bridge/src/session/values.rs.
"""

from __future__ import annotations

import base64
import binascii
import datetime as _dt
import decimal
import json
import math
import re
import uuid
from typing import Any, Dict, Iterable, TypeVar

TAGS = ("instant", "datetime", "date", "time", "bytes", "map", "set", "bigint", "decimal", "uuid")
# What this side declares: it takes every tagged value.
CAPABILITIES = [f"value-{tag}" for tag in TAGS]
# The most digits a bigint or decimal may have: Python's default limit on
# integer text.
MAX_DIGITS = 4300
# The largest plain integer crossing a session (#356): ±(2^53−1).
MAX_SAFE_INTEGER = 2**53 - 1

_K = TypeVar("_K")
_V = TypeVar("_V")


class BigInt(int):
    """A received big integer (tag `bigint`): an int to the plugin, which
    crosses as a bigint again even within ±(2^53−1); arithmetic on it gives
    a plain int. A plain int beyond ±(2^53−1) crosses as a bigint by itself
    (design V6)."""

    __slots__ = ()

    def __repr__(self) -> str:
        return f"BigInt({int.__repr__(self)})"

    def __str__(self) -> str:
        return int.__repr__(self)


class Map(Dict[_K, _V]):
    """A received map (tag `map`): a dict to the plugin, which crosses as a
    map again even when its keys are all strings (a plain dict with string
    keys crosses as an object). A dict with a key that is not a string
    crosses as a map by itself."""

    __slots__ = ()

    def __repr__(self) -> str:
        return f"Map({dict.__repr__(self)})"


class Refused(Exception):
    """A received value this side does not take: one not in its format, or
    a map or set whose keys break the rules. Only the call it came in
    fails; the session goes on."""


def tag_of(value: Any) -> str | None:
    """The tag `value` crosses under, or None for data, lists, dicts with
    string keys and references."""
    if isinstance(value, BigInt):
        return "bigint"
    if isinstance(value, int) and not isinstance(value, bool):
        # Beyond ±(2^53−1) an integer is a bigint (design V6).
        return None if -MAX_SAFE_INTEGER <= value <= MAX_SAFE_INTEGER else "bigint"
    if value is None or isinstance(value, (bool, float, str, list, tuple)):
        return None
    if isinstance(value, _dt.datetime):
        return "datetime" if value.utcoffset() is None else "instant"
    if isinstance(value, _dt.date):
        return "date"
    if isinstance(value, _dt.time):
        return "time"
    if isinstance(value, (bytes, bytearray, memoryview)):
        return "bytes"
    if isinstance(value, (set, frozenset)):
        return "set"
    if isinstance(value, Map):
        return "map"
    if isinstance(value, dict):
        return None if all(isinstance(key, str) for key in value) else "map"
    if isinstance(value, decimal.Decimal):
        return "decimal"
    if isinstance(value, uuid.UUID):
        return "uuid"
    return None


def describe(value: Any) -> str:
    """What `value` is, for errors: "None", "an instant", ..."""
    tag = tag_of(value)
    if tag == "instant":
        return "an instant (a datetime with a time zone)"
    if tag == "bytes":
        return "bytes"
    if tag is not None:
        return f"an {tag}" if tag[0] in "aeiou" else f"a {tag}"
    if value is None:
        return "None"
    if isinstance(value, bool):
        return "a boolean"
    if isinstance(value, (int, float)):
        return "a number"
    if isinstance(value, str):
        return "a string"
    if isinstance(value, (list, tuple)):
        return "a list"
    if isinstance(value, dict):
        return "a dict"
    return f"a reference ({type(value).__name__})"


def _quoted(text: str) -> str:
    """A text an error quotes: at most 40 characters."""
    if len(text) > 40:
        return f"{json.dumps(text[:40], ensure_ascii=False)}…"
    return json.dumps(text, ensure_ascii=False)


def _invalid(what: str, text: str, expected: str) -> Refused:
    return Refused(f"invalid {what} {_quoted(text)}: expected {expected}")


# ── Writing ──────────────────────────────────────────────────────────


def _fraction(microsecond: int) -> str:
    """`.` and the fraction's digits without trailing zeros; nothing for 0."""
    return f".{microsecond:06d}".rstrip("0") if microsecond else ""


def _date_text(value: _dt.date) -> str:
    return f"{value.year:04d}-{value.month:02d}-{value.day:02d}"


def _time_text(value: _dt.time | _dt.datetime) -> str:
    return f"{value.hour:02d}:{value.minute:02d}:{value.second:02d}{_fraction(value.microsecond)}"


def _digits(text: str) -> int:
    return sum(character.isdigit() for character in text)


def write(tag: str, value: Any) -> str:
    """The wire text of `value`, of a tag other than map and set; ValueError
    when it cannot be written without losing what it is."""
    if tag == "instant":
        try:
            utc = value.astimezone(_dt.timezone.utc)
        except OverflowError:
            raise ValueError(f"cannot send {value!r}: in UTC it is outside years 0001–9999") from None
        return f"{_date_text(utc)}T{_time_text(utc)}Z"
    if tag == "datetime":
        return f"{_date_text(value)}T{_time_text(value)}"
    if tag == "date":
        return _date_text(value)
    if tag == "time":
        if value.tzinfo is not None:
            raise ValueError(
                f"cannot send {value!r}: a time of day crosses without a time zone; "
                "send a datetime with the zone (an instant), or the time without it"
            )
        return _time_text(value)
    if tag == "bytes":
        return base64.b64encode(bytes(value)).decode("ascii")
    if tag == "bigint":
        if abs(value) >= 10**MAX_DIGITS:
            raise ValueError(f"cannot send the bigint: it has more than {MAX_DIGITS} digits")
        return int.__repr__(int(value))
    if tag == "decimal":
        if not value.is_finite():
            raise ValueError(f"cannot send Decimal({str(value)!r}): only finite decimals cross")
        _sign, digits, exponent = value.as_tuple()
        if len(digits) > MAX_DIGITS or not -MAX_DIGITS <= exponent <= MAX_DIGITS:
            raise ValueError(f"cannot send the decimal: it has more than {MAX_DIGITS} digits")
        # Without an exponent, its trailing zeros kept: 1.10 stays 1.10.
        text = format(value, "f")
        if _digits(text) > MAX_DIGITS:
            raise ValueError(f"cannot send the decimal: it has more than {MAX_DIGITS} digits")
        return text
    if tag == "uuid":
        return str(value)
    raise AssertionError(f"no text for {tag}")


# ── Reading ──────────────────────────────────────────────────────────

_DATE = r"([0-9]{4})-([0-9]{2})-([0-9]{2})"
_TIME = r"([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\.([0-9]{1,9}))?"
_DATE_TEXT = re.compile(_DATE)
_TIME_TEXT = re.compile(_TIME)
_DATETIME_TEXT = re.compile(f"{_DATE}T{_TIME}")
_INSTANT_TEXT = re.compile(f"{_DATE}T{_TIME}Z")
_INTEGER_TEXT = re.compile(r"-?(?:0|[1-9][0-9]*)")
_DECIMAL_TEXT = re.compile(r"-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?")
_UUID_TEXT = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")


def _microsecond(fraction: str | None) -> int:
    """The microseconds of a fraction of 1–9 digits, truncated (V7)."""
    return int((fraction or "").ljust(6, "0")[:6])


def _datetime(match: re.Match, tzinfo: _dt.tzinfo | None) -> _dt.datetime:
    year, month, day, hour, minute, second, fraction = match.groups()
    return _dt.datetime(
        int(year), int(month), int(day), int(hour), int(minute), int(second), _microsecond(fraction), tzinfo=tzinfo
    )


def read(tag: str, text: str) -> Any:
    """The value of wire text `text` of a tag other than map and set;
    Refused when it is not in the tag's format."""
    try:
        if tag == "instant":
            match = _INSTANT_TEXT.fullmatch(text)
            if match:
                return _datetime(match, _dt.timezone.utc)
            raise _invalid("instant", text, "RFC 3339 in UTC, YYYY-MM-DDTHH:MM:SS[.fraction]Z, years 0001–9999")
        if tag == "datetime":
            match = _DATETIME_TEXT.fullmatch(text)
            if match:
                return _datetime(match, None)
            raise _invalid(
                "datetime", text, "YYYY-MM-DDTHH:MM:SS[.fraction] without a time zone, years 0001–9999"
            )
        if tag == "date":
            match = _DATE_TEXT.fullmatch(text)
            if match:
                return _dt.date(*(int(part) for part in match.groups()))
            raise _invalid("date", text, "YYYY-MM-DD, years 0001–9999")
        if tag == "time":
            match = _TIME_TEXT.fullmatch(text)
            if match:
                hour, minute, second, fraction = match.groups()
                return _dt.time(int(hour), int(minute), int(second), _microsecond(fraction))
            raise _invalid("time", text, "HH:MM:SS[.fraction], 24-hour, 0–9 fraction digits")
        if tag == "bytes":
            # Only the canonical text: padded, no whitespace, unused bits zero.
            try:
                data = base64.b64decode(text, validate=True)
            except (binascii.Error, ValueError):
                data = None
            if data is None or base64.b64encode(data).decode("ascii") != text:
                raise Refused(
                    f"invalid bytes ({len(text)} characters): expected base64 (RFC 4648 §4) with padding"
                )
            return data
        if tag == "bigint":
            if _INTEGER_TEXT.fullmatch(text) and text != "-0" and _digits(text) <= MAX_DIGITS:
                return BigInt(int(text))
            raise _invalid("bigint", text, f"a decimal integer without leading zeros, at most {MAX_DIGITS} digits")
        if tag == "decimal":
            if _DECIMAL_TEXT.fullmatch(text) and _digits(text) <= MAX_DIGITS:
                return decimal.Decimal(text)
            raise _invalid(
                "decimal", text, f"decimal digits with an optional fraction, no exponent, at most {MAX_DIGITS} digits"
            )
        if tag == "uuid":
            if _UUID_TEXT.fullmatch(text):
                return uuid.UUID(text)
            raise _invalid("uuid", text, "lowercase hex 8-4-4-4-12")
    except ValueError as error:
        # No such day or hour, or Python's own limit on integer text.
        raise Refused(f"invalid {tag} {_quoted(text)}: {error}") from None
    raise AssertionError(f"no text for {tag}")


# ── Map keys and set elements (design §5) ────────────────────────────


def key_of(value: Any, what: str) -> tuple:
    """`value` as a map key or set element, as the session compares them:
    numbers by value (1 and 1.0 are one key, as are 0 and -0.0), the rest
    by kind and text. ValueError for a value no language takes as a key."""
    if isinstance(value, bool):
        return ("boolean", value)
    if isinstance(value, BigInt):
        return ("bigint", int(value))
    if isinstance(value, int):
        # Beyond ±(2^53−1) it crosses as a bigint (design V6).
        if not -MAX_SAFE_INTEGER <= value <= MAX_SAFE_INTEGER:
            return ("bigint", value)
        return ("number", value)
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ValueError(f"a {what} cannot be {value}: only finite numbers cross")
        return ("number", value)
    if isinstance(value, str):
        return ("string", value)
    tag = tag_of(value)
    if tag in ("datetime", "date", "time", "decimal", "uuid"):
        return (tag, write(tag, value))
    raise ValueError(
        f"a {what} cannot be {describe(value)}: only strings, finite numbers, booleans, bigint, datetime, "
        "date, time, decimal and uuid values can (an instant, bytes, None, a list, a dict, a map, a set "
        "or a reference cannot)"
    )


def _shown(value: Any) -> str:
    tag = tag_of(value)
    if tag in ("datetime", "date", "time", "decimal", "uuid", "bigint"):
        text = write(tag, value)
    else:
        text = json.dumps(value, ensure_ascii=False) if isinstance(value, (bool, int, float, str)) else describe(value)
    return f"{text[:40]}…" if len(text) > 40 else text


def check_keys(keys: Iterable[Any], what: str) -> None:
    """Check the keys of a map (`what` "map key") or the elements of a set
    ("set element"): each of a kind every language takes as a key, no two
    equal. ValueError otherwise."""
    seen: set = set()
    for value in keys:
        key = key_of(value, what)
        if key in seen:
            raise ValueError(f"duplicate {what} {_shown(value)}: a map or set whose keys collide would lose entries")
        seen.add(key)


def _received(keys: list, what: str) -> None:
    for key in keys:
        # A number on the wire beyond ±(2^53−1) is no key (§6.1): such an
        # integer is sent as a bigint, which arrives as a BigInt.
        if type(key) is int and not -MAX_SAFE_INTEGER <= key <= MAX_SAFE_INTEGER:
            raise Refused(f"a {what} cannot be the number {_shown(key)}, beyond ±(2^53-1): it is sent as a bigint")
    try:
        check_keys(keys, what)
    except ValueError as error:
        raise Refused(str(error)) from None


def _collides(value: Any, what: str) -> Refused:
    # Distinct on the wire, one key in Python: 1 and True, 1 and a bigint 1,
    # 1 and a decimal 1.0, two times a nanosecond apart.
    return Refused(
        f"{what} {_shown(value)} is equal in Python to an earlier one (as 1, True, Decimal('1.0') "
        f"and BigInt(1) are): the {'map' if what == 'map key' else 'set'} would lose an entry"
    )


def received_map(keys: list, values: list) -> Map:
    """The map with these decoded keys and values; Refused when the keys
    break the rules or two are equal in Python."""
    _received(keys, "map key")
    entries: Map = Map()
    for key, value in zip(keys, values):
        if key in entries:
            raise _collides(key, "map key")
        entries[key] = value
    return entries


def received_set(elements: list) -> set:
    """The set of these decoded elements; Refused as for map keys."""
    _received(elements, "set element")
    result: set = set()
    for element in elements:
        if element in result:
            raise _collides(element, "set element")
        result.add(element)
    return result

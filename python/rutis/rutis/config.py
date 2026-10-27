"""A row's config against its plugin's JSON Schema: defaults filled, then
validated (config schema design, docs/design-config-schema-2026-10-27.en.md
§3–§6). The rules every language's SDK has; the shared case list
specs/config-schema/cases.json keeps them the same word for word, and its
README states the details settled while implementing.

The runtime and the testing tool both call `configured` before a plugin's
`apply`, so a config that passes a test passes the runtime.

Keywords checked: type, properties, required, additionalProperties, items,
enum, const, minimum, maximum, default, anyOf, oneOf, allOf, $ref (to
#/$defs/… and #/definitions/…). Others are neither checked nor warned
about.
"""

from __future__ import annotations

import copy
import math
from dataclasses import dataclass, field
from decimal import Decimal
from typing import Any, Union

# At most this many problems are written; the rest as one last line.
SHOWN = 20
_TYPES = ("string", "number", "integer", "boolean", "object", "array", "null")
# Integers up to this magnitude are written without a decimal point.
_EXACT = 2**53

Segment = Union[str, int]


@dataclass
class ConfigIssue:
    """One problem: where it is (keys and indices), and its sentence, path
    included (`home.lat should be a number, got "north"`)."""

    path: list
    message: str


@dataclass
class Checked:
    """What checking a config gives: the config with defaults filled (what
    `apply` gets; None for no value), every problem sorted by path, and the
    extra keys where the schema allows them."""

    config: Any
    errors: list = field(default_factory=list)
    warnings: list = field(default_factory=list)


def issue(path: list, reason: str) -> ConfigIssue:
    """The problem at `path`, its sentence `<path> <reason>`."""
    return ConfigIssue(list(path), f"{path_text(path)} {reason}")


def check(schema: Any, config: Any = None, *, required: bool = True) -> Checked:
    """Fill `config` (None: no config) with the defaults of `schema`, then
    validate it. `config` is copied: changing the result changes nothing of
    it. `required`: missing `required` keys are errors."""
    doc = _Document(schema)
    none = config is None
    if not none:
        value = copy.deepcopy(config)
    else:
        default = doc.default_of([schema])
        if default is not _MISSING:
            value = copy.deepcopy(default)
        elif doc.is_object_root(schema):
            value = {}
        else:
            value = None
    doc.fill([schema], value)
    sink = _Sink(warn=True)
    value = doc.validate([schema], value, [], required, sink, {})
    # No value where the root does not allow it.
    if value is None and none and sink.errors:
        sink.errors = [issue([], "is required")]
    return Checked(value, _sorted(sink.errors), _sorted(sink.warnings))


def config_lines(issues: list) -> list:
    """The lines a config error is written as: `config: <problem>`, at most
    20, then `config: and N more problems`."""
    lines = [f"config: {problem.message}" for problem in issues[:SHOWN]]
    if len(issues) > SHOWN:
        lines.append(f"config: and {len(issues) - SHOWN} more problems")
    return lines


def config_error(issues: list) -> BaseException:
    """The error a row fails with when its config does not match: a
    BindingError with code `bad-config`, its message the lines of
    `config_lines`, its data `{issues}` (design §5.2)."""
    from .peer import BindingError

    error = BindingError("\n".join(config_lines(issues)))
    error.code = "bad-config"
    error.data = {"issues": [{"path": list(problem.path), "message": problem.message} for problem in issues]}
    return error


def configured(schema: Any, config: Any, *, warn: Any = None) -> Any:
    """The config a plugin's `apply` gets: without a schema, `config` as
    given; with one, a copy with the defaults filled, once it passes.
    Raises `config_error` otherwise. `warn(issue)` is told of each extra
    key (the testing tool; in a runtime the host's loader warns)."""
    if schema is None:
        return config
    checked = check(schema, config)
    if warn is not None:
        for warning in checked.warnings:
            warn(warning)
    if checked.errors:
        raise config_error(checked.errors)
    return checked.config


# ── How paths and values are written (design §5.1, §6.2, §6.3) ──────


def path_text(path: list) -> str:
    """`home.lat`, `cities[2]`, `headers["x-api"]`; `config` for the root
    (and before a first segment that is not an identifier key)."""
    out = []
    for index, segment in enumerate(path):
        if isinstance(segment, str) and _identifier(segment):
            if index > 0:
                out.append(".")
            out.append(segment)
        elif isinstance(segment, str):
            if index == 0:
                out.append("config")
            out.append(f"[{_quoted(segment, None)}]")
        else:
            if index == 0:
                out.append("config")
            out.append(f"[{segment}]")
    return "".join(out) or "config"


def _identifier(key: str) -> bool:
    """`[A-Za-z_][A-Za-z0-9_]*`."""
    if not key or not (_ascii_alpha(key[0]) or key[0] == "_"):
        return False
    return all(_ascii_alpha(c) or "0" <= c <= "9" or c == "_" for c in key[1:])


def _ascii_alpha(c: str) -> bool:
    return "a" <= c <= "z" or "A" <= c <= "Z"


def value_text(value: Any) -> str:
    """How a value is written after `got` (and in `enum` lists): strings,
    numbers, booleans and `null` as JSON, a string past 40 scalar values cut
    with `…`; `an object`, `an array of N`."""
    if value is None:
        return "null"
    if value is True:
        return "true"
    if value is False:
        return "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return float_text(value)
    if isinstance(value, str):
        return _quoted(value, 40)
    if isinstance(value, (list, tuple)):
        return f"an array of {len(value)}"
    return "an object"


def float_text(f: float) -> str:
    """`f` as JavaScript's `String(number)` writes it; an integral value of
    magnitude at most 2^53 without a decimal point."""
    if not math.isfinite(f):
        return "NaN" if math.isnan(f) else ("Infinity" if f > 0 else "-Infinity")
    if f == 0:
        return "0"
    if f.is_integer() and abs(f) <= _EXACT:
        return str(int(f))
    # The shortest round-trip digits (repr's) and the position of the
    # decimal point, as JavaScript's algorithm writes them.
    _, digits, exponent = Decimal(repr(abs(f))).as_tuple()
    text = "".join(map(str, digits)).rstrip("0") or "0"
    stripped = len(digits) - len(text)
    k = len(text)
    n = exponent + stripped + k
    sign = "-" if f < 0 else ""
    if k <= n <= 21:
        body = text + "0" * (n - k)
    elif 0 < n <= 21:
        body = f"{text[:n]}.{text[n:]}"
    elif -6 < n <= 0:
        body = f"0.{'0' * -n}{text}"
    else:
        e = n - 1
        mark = f"e+{e}" if e >= 0 else f"e-{-e}"
        body = f"{text}{mark}" if k == 1 else f"{text[0]}.{text[1:]}{mark}"
    return sign + body


_ESCAPES = {'"': '\\"', "\\": "\\\\", "\n": "\\n", "\r": "\\r", "\t": "\\t", "\b": "\\b", "\f": "\\f"}


def _quoted(s: str, limit: int | None) -> str:
    """`s` as a JSON string, its content cut to `limit` scalar values with
    `…`; `"`, `\\` and control characters escaped, everything else as it
    is."""
    shown = s if limit is None else s[:limit]
    out = ['"']
    for c in shown:
        if c in _ESCAPES:
            out.append(_ESCAPES[c])
        elif ord(c) < 0x20:
            out.append(f"\\u{ord(c):04x}")
        else:
            out.append(c)
    if limit is not None and len(s) > limit:
        out.append("…")
    out.append('"')
    return "".join(out)


# ── Spelling suggestions (design §6.1) ──────────────────────────────


def closest(name: str, known: list) -> str | None:
    """The key of `known` (sorted by code point) closest to `name`, when it
    is close enough to be a misspelling of it: none for a name of 0–2
    scalar values, at most one edit for 3–6, two for longer."""
    count = len(name)
    if count <= 2:
        return None
    limit = 1 if count <= 6 else 2
    best = None
    for candidate in known:
        if candidate == name:
            continue
        d = distance(name, candidate)
        if d <= limit and (best is None or d < best[0]):
            best = (d, candidate)
    return best[1] if best else None


def distance(a: str, b: str) -> int:
    """Edits (insert, delete, replace, swap two neighbours) between `a` and
    `b`, by scalar value."""
    rows = [[0] * (len(b) + 1) for _ in range(len(a) + 1)]
    for i in range(len(a) + 1):
        rows[i][0] = i
    for j in range(len(b) + 1):
        rows[0][j] = j
    for i in range(1, len(a) + 1):
        for j in range(1, len(b) + 1):
            cost = 0 if a[i - 1] == b[j - 1] else 1
            best = min(rows[i - 1][j] + 1, rows[i][j - 1] + 1, rows[i - 1][j - 1] + cost)
            if i > 1 and j > 1 and a[i - 1] == b[j - 2] and a[i - 2] == b[j - 1]:
                best = min(best, rows[i - 2][j - 2] + 1)
            rows[i][j] = best
    return rows[len(a)][len(b)]


# ── The checking ────────────────────────────────────────────────────

_MISSING = object()


def _sort_key(problem: ConfigIssue) -> list:
    # Segment by segment, a key before an index, keys by code point,
    # indices by value; a prefix first.
    return [(0, segment) if isinstance(segment, str) else (1, segment) for segment in problem.path]


def _sorted(issues: list) -> list:
    """Problems sorted by path (stable: at one path, in the order found),
    each once."""
    issues = sorted(issues, key=_sort_key)
    seen = set()
    out = []
    for problem in issues:
        mark = (tuple(problem.path), problem.message)
        if mark not in seen:
            seen.add(mark)
            out.append(problem)
    return out


@dataclass
class _Sink:
    errors: list = field(default_factory=list)
    warnings: list = field(default_factory=list)
    # Extra keys are warned about (not inside anyOf / oneOf branches).
    warn: bool = False


# The state of an anyOf / oneOf branch tried at one value.
_TRYING = object()
_FAILED = object()


def _index(token: str) -> int | None:
    """An array index in a JSON Pointer: digits without a leading zero."""
    if token.isascii() and token.isdigit() and (token == "0" or not token.startswith("0")):
        return int(token)
    return None


class _Document:
    """The schema document `$ref`s resolve in."""

    def __init__(self, root: Any):
        self.root = root

    def resolve(self, reference: Any) -> Any:
        """`#/$defs/…` or `#/definitions/…` in the document; _MISSING."""
        if not isinstance(reference, str) or not reference.startswith("#"):
            return _MISSING
        pointer = reference[1:]
        if not (pointer.startswith("/$defs/") or pointer.startswith("/definitions/")):
            return _MISSING
        node = self.root
        for token in pointer[1:].split("/"):
            token = token.replace("~1", "/").replace("~0", "~")
            if isinstance(node, dict) and token in node:
                node = node[token]
            elif isinstance(node, list) and _index(token) is not None and _index(token) < len(node):
                node = node[int(token)]
            else:
                return _MISSING
        return node

    def parts(self, schemas: list) -> list:
        """`schemas` and what they take in through `$ref` and `allOf`, each
        once: a schema, then its `$ref`'s, then its `allOf` members' in
        order."""
        out: list = []
        seen: set = set()
        for schema in schemas:
            self._collect(schema, out, seen)
        return out

    def _collect(self, schema: Any, out: list, seen: set) -> None:
        if not isinstance(schema, dict) or id(schema) in seen:
            return
        seen.add(id(schema))
        out.append(schema)
        target = self.resolve(schema.get("$ref"))
        if target is not _MISSING:
            self._collect(target, out, seen)
        members = schema.get("allOf")
        if isinstance(members, list):
            for member in members:
                self._collect(member, out, seen)

    def default_of(self, schemas: list) -> Any:
        """The first `default` of `schemas` (through `$ref` and `allOf`);
        _MISSING."""
        for part in self.parts(schemas):
            if "default" in part:
                return part["default"]
        return _MISSING

    def is_object_root(self, schema: Any) -> bool:
        """The root's type is `object` or includes it."""
        for part in self.parts([schema]):
            if "type" in part:
                return "object" in _types(part["type"])
        return False

    @staticmethod
    def property(parts: list, key: str) -> list:
        """The schemas a child at `key` is checked against: each part's
        `properties[key]`."""
        out = []
        for part in parts:
            properties = part.get("properties")
            if isinstance(properties, dict) and key in properties:
                out.append(properties[key])
        return out

    @staticmethod
    def additional(parts: list) -> Any:
        """The first `additionalProperties` of `parts`; _MISSING."""
        for part in parts:
            if "additionalProperties" in part:
                return part["additionalProperties"]
        return _MISSING

    @staticmethod
    def known(parts: list) -> list:
        """Every `properties` key of `parts`, sorted by code point."""
        keys = set()
        for part in parts:
            properties = part.get("properties")
            if isinstance(properties, dict):
                keys.update(properties)
        return sorted(keys)

    def fill(self, schemas: list, value: Any) -> None:
        """Fill defaults level by level: in an object that is there, each
        missing property with a `default`; then into the child objects and
        array items. Not into anyOf / oneOf."""
        parts = self.parts(schemas)
        if isinstance(value, dict):
            for part in parts:
                properties = part.get("properties")
                if not isinstance(properties, dict):
                    continue
                for key, schema in properties.items():
                    if key in value:
                        continue
                    default = self.default_of([schema])
                    if default is not _MISSING:
                        value[key] = copy.deepcopy(default)
            known = set(self.known(parts))
            additional = self.additional(parts)
            for key, child in value.items():
                if key in known:
                    below = self.property(parts, key)
                elif isinstance(additional, dict):
                    below = [additional]
                else:
                    below = []
                if below:
                    self.fill(below, child)
        elif isinstance(value, list):
            below = [part["items"] for part in parts if isinstance(part.get("items"), dict)]
            if below:
                for item in value:
                    self.fill(below, item)

    def validate(self, schemas: list, value: Any, path: list, required: bool, sink: _Sink, tried: dict) -> Any:
        """Validate `value` at `path` against every one of `schemas`; the
        value as it passed (integers normalised, a branch's defaults)."""
        parts = self.parts(schemas)
        # The type first: when it is wrong, nothing else at or below it.
        for part in parts:
            if "type" not in part:
                continue
            names = _types(part["type"])
            if not names:
                continue
            if not any(_is_type(name, value) for name in names):
                expected = [_type_text(name) for name in names]
                sink.errors.append(issue(path, f"should be {_joined(expected)}, got {value_text(value)}"))
                return value
            if "integer" in names:
                value = _integral(value)
        for part in parts:
            allowed = part.get("enum")
            if isinstance(allowed, list) and not any(_same(candidate, value) for candidate in allowed):
                texts = [value_text(candidate) for candidate in allowed]
                sink.errors.append(issue(path, f"should be {_joined(texts)}, got {value_text(value)}"))
            if "const" in part and not _same(part["const"], value):
                sink.errors.append(issue(path, f"should be {value_text(part['const'])}, got {value_text(value)}"))
            if _is_number(value):
                minimum = part.get("minimum")
                if _is_number(minimum) and value < minimum:
                    sink.errors.append(issue(path, f"should be at least {value_text(minimum)}, got {value_text(value)}"))
                maximum = part.get("maximum")
                if _is_number(maximum) and value > maximum:
                    sink.errors.append(issue(path, f"should be at most {value_text(maximum)}, got {value_text(value)}"))
            for keyword in ("anyOf", "oneOf"):
                branches = part.get(keyword)
                if not isinstance(branches, list):
                    continue
                passed = _MISSING
                for branch in branches:
                    # At one value each branch is tried once: one being
                    # tried (reached again through $ref) does not match,
                    # one tried before gives its result again.
                    state = tried.get(id(branch), _MISSING)
                    if state is _TRYING or state is _FAILED:
                        result = _MISSING
                    elif state is not _MISSING:
                        result = copy.deepcopy(state[0])
                    else:
                        trial = _Sink()
                        tried[id(branch)] = _TRYING
                        candidate = self.validate([branch], copy.deepcopy(value), path, required, trial, tried)
                        result = _MISSING if trial.errors else candidate
                        tried[id(branch)] = _FAILED if result is _MISSING else (copy.deepcopy(result),)
                    if result is not _MISSING:
                        passed = result
                        break
                if passed is not _MISSING:
                    value = passed
                else:
                    texts = self.branch_texts(branches)
                    sink.errors.append(issue(path, f"should be {_joined(texts)}, got {value_text(value)}"))
        if isinstance(value, dict):
            if required:
                for part in parts:
                    keys = part.get("required")
                    if not isinstance(keys, list):
                        continue
                    for key in keys:
                        if isinstance(key, str) and key not in value:
                            sink.errors.append(issue([*path, key], "is required"))
            known = self.known(parts)
            known_set = set(known)
            additional = self.additional(parts)
            described = any("properties" in part for part in parts)
            for key in list(value):
                at = [*path, key]
                if key in known_set:
                    value[key] = self.validate(self.property(parts, key), value[key], at, required, sink, {})
                elif additional is False:
                    sink.errors.append(_extra(at, key, known))
                elif isinstance(additional, dict):
                    value[key] = self.validate([additional], value[key], at, required, sink, {})
                elif additional is _MISSING and described and sink.warn:
                    sink.warnings.append(_extra(at, key, known))
        elif isinstance(value, list):
            below = [part["items"] for part in parts if isinstance(part.get("items"), dict)]
            if below:
                for index, item in enumerate(value):
                    value[index] = self.validate(below, item, [*path, index], required, sink, {})
        return value

    def branch_texts(self, branches: list) -> list:
        """How the branches of a failed anyOf / oneOf are written (design
        §6.5), in order, each once."""
        texts: list = []
        for branch in branches:
            found = None
            for part in self.parts([branch]):
                if "const" in part:
                    found = [value_text(part["const"])]
                    break
                if isinstance(part.get("enum"), list):
                    found = [value_text(candidate) for candidate in part["enum"]]
                    break
                if "type" in part:
                    names = _types(part["type"])
                    if names:
                        found = [_type_text(name) for name in names]
                        break
            for text in found or ["another accepted value"]:
                if text not in texts:
                    texts.append(text)
        return texts


def _extra(path: list, key: str, known: list) -> ConfigIssue:
    """`<path> is not a config key`, with a close known key suggested."""
    like = closest(key, known)
    return issue(path, f"is not a config key (did you mean {like}?)" if like is not None else "is not a config key")


def _types(ty: Any) -> list:
    """The type names of `type` this module knows."""
    names = [ty] if isinstance(ty, str) else [name for name in ty if isinstance(name, str)] if isinstance(ty, list) else []
    return [name for name in names if name in _TYPES]


def _is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _is_type(name: str, value: Any) -> bool:
    if name == "string":
        return isinstance(value, str)
    if name == "number":
        return _is_number(value)
    if name == "integer":
        return _is_number(value) and (isinstance(value, int) or (math.isfinite(value) and value.is_integer()))
    if name == "boolean":
        return isinstance(value, bool)
    if name == "object":
        return isinstance(value, dict)
    if name == "array":
        return isinstance(value, list)
    return value is None


def _integral(value: Any) -> Any:
    """An integral float written as an integer."""
    if isinstance(value, float) and value.is_integer() and abs(value) <= _EXACT:
        return int(value)
    return value


def _type_text(name: str) -> str:
    return {
        "string": "a string",
        "number": "a number",
        "integer": "an integer",
        "boolean": "a boolean",
        "object": "an object",
        "array": "an array",
    }.get(name, "null")


def _joined(texts: list) -> str:
    """`a`, `a or b`, `one of a, b, c`."""
    if len(texts) == 1:
        return texts[0]
    if len(texts) == 2:
        return f"{texts[0]} or {texts[1]}"
    return f"one of {', '.join(texts)}"


def _same(a: Any, b: Any) -> bool:
    """JSON equality: numbers by value, objects regardless of key order; a
    boolean is not a number."""
    if isinstance(a, bool) or isinstance(b, bool):
        return isinstance(a, bool) and isinstance(b, bool) and a == b
    if _is_number(a) and _is_number(b):
        return a == b
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(_same(x, y) for x, y in zip(a, b))
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(_same(a[key], b[key]) for key in a)
    if type(a) is not type(b):
        return False
    return a == b

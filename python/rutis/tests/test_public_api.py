"""The public API of the `rutis` package, locked by `public-api.txt` (#692,
SDK entry design §6): the `__all__` of `rutis`, `rutis.advanced` and
`rutis.testing`, each class's public members and every signature. Adding
or changing a name changes the text, and review sees it.

To accept an intended change:

    RUTIS_PUBLIC_API_WRITE=1 python3 -m unittest tests.test_public_api
"""

import difflib
import enum
import importlib
import inspect
import os
import types
import unittest
from pathlib import Path
from typing import ForwardRef

SNAPSHOT = Path(__file__).resolve().parent.parent / "public-api.txt"
MODULES = ("rutis", "rutis.advanced", "rutis.testing")
# Names in the root's `__all__` that moved to rutis.advanced in 0.19 (#692);
# listed in their own section, removed when the plugin API version changes.
MOVED = "rutis.advanced"


def annotation(value) -> str:
    # The same text on every supported Python (3.10 to 3.14): an annotation
    # written as a string, evaluated or deferred, prints unquoted.
    if isinstance(value, str):
        return value
    if isinstance(value, ForwardRef):
        return value.__forward_arg__
    return inspect.formatannotation(value)


def signature(value) -> str:
    try:
        sig = inspect.signature(value)
    except (TypeError, ValueError):
        return "(…)"
    parts = []
    kind = None
    for param in sig.parameters.values():
        if param.kind is param.KEYWORD_ONLY and kind not in (param.KEYWORD_ONLY, param.VAR_POSITIONAL):
            parts.append("*")
        if kind is param.POSITIONAL_ONLY and param.kind is not param.POSITIONAL_ONLY:
            parts.append("/")
        kind = param.kind
        text = {param.VAR_POSITIONAL: "*", param.VAR_KEYWORD: "**"}.get(param.kind, "") + param.name
        if param.annotation is not param.empty:
            text += f": {annotation(param.annotation)}"
        if param.default is not param.empty:
            text += " = " + ("<factory>" if "factory" in repr(param.default) else repr(param.default))
        parts.append(text)
    if kind is inspect.Parameter.POSITIONAL_ONLY:
        parts.append("/")
    returns = "" if sig.return_annotation is sig.empty else f" -> {annotation(sig.return_annotation)}"
    return f"({', '.join(parts)}){returns}"


def member(name: str, value) -> str:
    if isinstance(value, staticmethod):
        return f"@staticmethod def {name}{signature(value.__func__)}"
    if isinstance(value, classmethod):
        return f"@classmethod def {name}{signature(value.__func__)}"
    if isinstance(value, property):
        returns = inspect.signature(value.fget).return_annotation if value.fget else inspect.Signature.empty
        shown = "" if returns is inspect.Signature.empty else f": {annotation(returns)}"
        return f"property {name}{shown}"
    if inspect.iscoroutinefunction(value):
        return f"async def {name}{signature(value)}"
    if inspect.isfunction(value):
        return f"def {name}{signature(value)}"
    return f"{name} = {value!r}" if isinstance(value, (int, str, float, bool)) else name


def describe(name: str, value) -> list[str]:
    if inspect.isclass(value):
        bases = ", ".join(f"{base.__module__}.{base.__qualname__}" if not base.__module__ == "builtins" else base.__qualname__ for base in value.__bases__)
        lines = [f"class {name}({bases})"]
        if not issubclass(value, enum.Enum):
            lines[0] += signature(value)
        seen = set()
        for klass in value.__mro__:
            if not klass.__module__.startswith("rutis"):
                continue
            for field, hint in getattr(klass, "__annotations__", {}).items():
                if not field.startswith("_") and field not in seen:
                    seen.add(field)
                    lines.append(f"    {field}: {annotation(hint)}")
            for field, attribute in vars(klass).items():
                if not field.startswith("_") and field not in seen:
                    seen.add(field)
                    lines.append(f"    {member(field, attribute)}")
        return lines
    if inspect.iscoroutinefunction(value):
        return [f"async def {name}{signature(value)}"]
    if inspect.isfunction(value):
        return [f"def {name}{signature(value)}"]
    return [f"{name}: {type(value).__name__} = {value!r}"]


def current() -> str:
    lines = []
    for module_name in MODULES:
        module = importlib.import_module(module_name)
        names = list(module.__all__)
        moved = []
        if module_name == "rutis":
            advanced = importlib.import_module(MOVED)
            moved = [name for name in names if name in advanced.__all__]
            names = [name for name in names if name not in moved]
        lines.append(f"## {module_name}")
        for name in sorted(names):
            lines.extend(describe(name, getattr(module, name)))
        if moved:
            lines.append(f"## {module_name} (deprecated: moved to {MOVED} in 0.19)")
            lines.extend(f"{name} -> {MOVED}.{name}" for name in sorted(moved))
        lines.append("")
    return "\n".join(lines)


class PublicApi(unittest.TestCase):
    def test_the_public_api_is_the_snapshot(self):
        text = current()
        if os.environ.get("RUTIS_PUBLIC_API_WRITE") == "1":
            SNAPSHOT.write_text(text, encoding="utf-8")
        expected = SNAPSHOT.read_text(encoding="utf-8") if SNAPSHOT.exists() else ""
        if text != expected:
            diff = "".join(difflib.unified_diff(
                expected.splitlines(keepends=True), text.splitlines(keepends=True),
                "python/rutis/public-api.txt", "current"))
            self.fail(f"\n{diff}the public API of rutis changed: review the diff, "
                      "then run RUTIS_PUBLIC_API_WRITE=1 on tests/test_public_api.py")

    def test_every_public_name_a_module_defines_is_in_all(self):
        # Names a module imports for its own use (helpers, typing) are not
        # its API; what it defines itself is, and must be in `__all__`.
        for module_name in MODULES:
            module = importlib.import_module(module_name)
            for name in dir(module):
                value = getattr(module, name) if name not in getattr(module, "_MOVED", ()) else None
                if name.startswith("_") or name in module.__all__ or isinstance(value, types.ModuleType):
                    continue
                with self.subTest(module=module_name, name=name):
                    self.assertNotEqual(getattr(value, "__module__", None), module_name,
                                        f"{module_name}.{name} is public but not in __all__")

    def test_the_root_names_are_the_plugin_author_s(self):
        import rutis
        import rutis.advanced

        root = [name for name in rutis.__all__ if name not in rutis.advanced.__all__]
        self.assertEqual(sorted(root), ["BindingError", "Context", "Plugin", "RemoteError", "define_plugin"])


if __name__ == "__main__":
    unittest.main()

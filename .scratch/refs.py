"""Python test references in docs that no longer exist."""
import ast
import pathlib
import re

ROOT = pathlib.Path("/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37")
tests = ROOT / "python/rutis/tests"
names = {}
for f in tests.glob("*.py"):
    tree = ast.parse(f.read_text())
    found = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            found.add(node.name)
    names[f.name] = found
for doc in ["docs/guarantees.md", "docs/guarantees.en.md", "docs/faults.md", "docs/faults.en.md", "docs/quality-status.md", "docs/quality-status.en.md"]:
    p = ROOT / doc
    if not p.exists():
        continue
    for i, line in enumerate(p.read_text().splitlines(), 1):
        for m in re.finditer(r"python/rutis/tests/([a-z_]+\.py)((?:::[A-Za-z_]+)*)", line):
            file, rest = m.group(1), m.group(2)
            if file not in names:
                print(f"{doc}:{i}: missing file {file}")
                continue
            for part in [x for x in rest.split("::") if x]:
                if part not in names[file]:
                    print(f"{doc}:{i}: missing {file}::{part}")
        # `::test_x` continuation names after a python ref
        if "python/rutis/tests/" in line:
            for m in re.finditer(r"`::(test_[a-z_0-9]+|[A-Z][A-Za-z]+::test_[a-z_0-9]+)`", line):
                n = m.group(1).split("::")[-1]
                if not any(n in v for v in names.values()):
                    print(f"{doc}:{i}: missing ::{m.group(1)}")

"""Write the Python examples' files and run their unittest steps on this checkout."""
import json
import os
import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path("/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37")
examples = json.load(open(ROOT / ".scratch/examples.json"))
for example in examples:
    name = example["name"]
    if name not in ("python", "python-en", "plugin-api-py", "plugin-api-py-en"):
        continue
    base = ROOT / ".scratch/ex" / name
    shutil.rmtree(base, ignore_errors=True)
    base.mkdir(parents=True)
    for step in sorted(example["steps"], key=lambda s: s["position"]):
        if step.get("file"):
            path = base / step["file"]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(step["body"])
        elif "unittest" in step["body"]:
            cwd = base / "greeter"
            if not (cwd / "tests").exists():
                continue
            env = dict(os.environ, RUTIS_HOST=str(ROOT / "target/debug/rutis-host"),
                       PYTHONPATH=f"{ROOT}/python/rutis:{cwd}/src:{cwd}")
            for _ in range(5):
                r = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], cwd=cwd, env=env, capture_output=True, text=True)
                print(name, r.returncode, r.stderr.strip().splitlines()[-1:] if r.returncode == 0 else r.stderr[-1500:])

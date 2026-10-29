import os
import shutil
import subprocess
import sys
import time

root = "/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37"
shutil.copy(f"{root}/crates/rutis-host/templates/python/src/__module__/__init__.py", f"{root}/.scratch/tpl/greeter/__init__.py")
env = dict(os.environ, RUTIS_HOST=f"{root}/target/debug/rutis-host", PYTHONPATH=f"{root}/python/rutis:.")
for _ in range(2):
    t = time.time()
    r = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], cwd=f"{root}/.scratch/tpl", env=env, capture_output=True, text=True)
    print(r.stderr.strip().splitlines()[-3:], f"{time.time() - t:.2f}s")

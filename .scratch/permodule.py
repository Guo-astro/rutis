import glob, os, subprocess, sys, time
os.chdir("/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37/python/rutis")
os.environ["RUTIS_HOST"] = "/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37/target/debug/rutis-host"
files = sys.argv[1:] or sorted(os.path.basename(f) for f in glob.glob("tests/test_*.py"))
total = 0
for f in files:
    t = time.time()
    try:
        r = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", f], capture_output=True, text=True, timeout=120)
        out = r.stderr
        last = [l for l in out.splitlines() if l.startswith(("Ran", "OK", "FAILED"))]
        status = " ".join(last)
    except subprocess.TimeoutExpired as e:
        status = "TIMEOUT"
        out = (e.stderr or b"").decode() if isinstance(e.stderr, bytes) else (e.stderr or "")
    d = time.time() - t
    total += d
    print(f"{f:32} {d:6.1f}s {status}")
    if "OK" not in status:
        open(f"/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37/.scratch/out-{f}.txt", "w").write(out)
print(f"total {total:.1f}s")

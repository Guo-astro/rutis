"""The dial conformance plugin (crates/rutis-host/tests/dial_conformance.rs),
for the Python runtime: it says on stderr when its row loads and unloads,
with its config, so the suite sees each session's rows come and go."""
import sys


def apply(ctx, config):
    print(f"conformance: loaded {config}", file=sys.stderr, flush=True)
    return lambda: print(f"conformance: unloaded {config}", file=sys.stderr, flush=True)

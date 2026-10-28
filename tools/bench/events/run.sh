#!/usr/bin/env bash
# Event dispatch benchmark (docs/performance-events-2026-10-28.md).
#
#   tools/bench/events/run.sh [node] [python] [dsh]    # default: node python dsh
#
# node, python: a host with one row in that runtime; the row emits to its own
# listeners with `emit`, `serial` and synchronous `bail`, 10 000 times each
# after 1 000 warm-up rounds, and reports the median and p99 in microseconds.
# dsh: the wall time of one run of the dsh baseline test
# (rutis-bridge tests/dsh_baseline.rs, scenarios in node/baseline), five runs.
#
# Environment:
#   RUTIS_HOST             rutis-host executable (default: build it with cargo)
#   RUTIS_BENCH_PROFILE    cargo profile for that build (default: release)
#   RUTIS_PYTHON           Python interpreter (default: the host's own choice)
#   RUTIS_BENCH_ITERATIONS, RUTIS_BENCH_WARMUP   (default: 10000, 1000)
#
# Needs `npm --prefix node/rutis-runtime ci`; dsh also `npm --prefix node/baseline ci`.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
targets=("$@")
test ${#targets[@]} -gt 0 || targets=(node python dsh)

host() {
  if test -n "${RUTIS_HOST:-}"; then echo "$RUTIS_HOST"; return; fi
  local profile="${RUTIS_BENCH_PROFILE:-release}" dir
  cargo build -q -p rutis-host --profile "$profile" --manifest-path "$repo/Cargo.toml" >&2
  dir="$profile"; test "$profile" = dev && dir=debug
  echo "${CARGO_TARGET_DIR:-$repo/target}/$dir/rutis-host"
}

# One host, one row; prints the row's report.
events() {
  local language="$1" executable="$2" work out row runtimes pid
  work="$(mktemp -d "${TMPDIR:-/tmp}/rutis-bench-events.XXXXXX")"
  out="$work/report.json"
  case "$language" in
    node)
      test -d "$repo/node/rutis-runtime/node_modules" \
        || { echo "run: npm --prefix node/rutis-runtime ci" >&2; exit 1; }
      row="$here/bench.mjs"
      runtimes="\"node\": { \"project\": \"$here\", \"runtime\": \"$repo/node/rutis-runtime\" }" ;;
    python)
      export RUTIS_PYTHON_PATH="${RUTIS_PYTHON_PATH:-$repo/python/rutis}"
      row="py:bench_events"
      runtimes="\"py\": { \"project\": \"$here\"${RUTIS_PYTHON:+, \"python\": \"$RUTIS_PYTHON\"} }" ;;
  esac
  cat > "$work/rutis.json" <<EOF
{
  "id": "bench",
  "runtimes": { $runtimes },
  "rows": [{ "id": "bench", "name": "$row", "config": {
    "out": "$out",
    "iterations": ${RUTIS_BENCH_ITERATIONS:-10000},
    "warmup": ${RUTIS_BENCH_WARMUP:-1000}
  } }]
}
EOF
  "$executable" run "$work/rutis.json" > "$work/host.log" 2>&1 &
  pid=$!
  for _ in $(seq 600); do
    test -s "$out" && break
    kill -0 "$pid" 2>/dev/null && ! grep -q "unresolved\|failed" "$work/host.log" || break
    sleep 0.5
  done
  kill -TERM "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  if ! test -s "$out"; then
    echo "$language: no report; host output:" >&2
    cat "$work/host.log" >&2
    exit 1
  fi
  cat "$out"; echo
  if grep -q '"error"' "$out"; then exit 1; fi
  rm -rf "$work"
}

dsh() {
  test -d "$repo/node/baseline/node_modules" \
    || { echo "run: npm --prefix node/baseline ci" >&2; exit 1; }
  local binary
  binary="$(cargo test -q -p rutis-bridge --test dsh_baseline --no-run --message-format=json \
    --manifest-path "$repo/Cargo.toml" \
    | python3 -c 'import json,sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "dsh_baseline":
        print(m["executable"])')"
  python3 - "$binary" "$repo/node/baseline" <<'PY'
import json, statistics, subprocess, sys, tempfile, time
binary, baseline = sys.argv[1:]
def timed(command, **kw):
    start = time.perf_counter()
    subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **kw)
    return (time.perf_counter() - start) * 1000
whole = [timed([binary, "--exact", "published_plugins_match_native_cordis"]) for _ in range(5)]
native = []
for _ in range(5):
    with tempfile.TemporaryDirectory() as root:
        subprocess.run(["node", "scenarios.mjs", root], cwd=baseline, check=True, stdout=subprocess.DEVNULL)
        native.append(timed(["node", "native.mjs", root], cwd=baseline))
print(json.dumps({
    "dsh_baseline_test_ms": {"median": round(statistics.median(whole)), "runs": [round(x) for x in whole]},
    "native_cordis_ms": {"median": round(statistics.median(native)), "runs": [round(x) for x in native]},
}, indent=2))
PY
}

executable=""
for target in "${targets[@]}"; do
  case "$target" in
    node|python)
      test -n "$executable" || executable="$(host)"
      events "$target" "$executable" ;;
    dsh) dsh ;;
    *) echo "unknown target $target (node, python, dsh)" >&2; exit 2 ;;
  esac
done

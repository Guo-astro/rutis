#!/usr/bin/env bash
# Run a test command and fail it when it stops producing output, naming what
# was running: a test that hangs keeps a job for its whole timeout, and its
# failure is never printed (#1402).
#
#   bash tools/test-watchdog.sh cargo test -p rutis-loader --all-features
#
# The command's output goes to the job's console as it comes, and is kept so
# that the last lines can be printed when the command stops: libtest prints
# `test <name> has been running for over 60 seconds` for a test that is still
# running, so the naming comes from there.
#
# Environment:
#   RUTIS_TEST_STALL   seconds without output before the run is failed
#                      (default 300; the longest silence a passing CI job has
#                      shown is 84 s, and a hung test prints once at 60 s)
#   RUTIS_TEST_LIMIT   seconds for the whole command, 0 for none (default 0)
#
# Exit status: the command's, or 124 when the watchdog ends it.

set -uo pipefail

stall=${RUTIS_TEST_STALL:-300}
limit=${RUTIS_TEST_LIMIT:-0}
if ! [[ $stall =~ ^[0-9]+$ ]] || ! [[ $limit =~ ^[0-9]+$ ]]; then
  echo "test-watchdog: RUTIS_TEST_STALL and RUTIS_TEST_LIMIT are seconds" >&2
  exit 2
fi
if [ $# -eq 0 ]; then
  echo "usage: bash tools/test-watchdog.sh <command...>" >&2
  exit 2
fi

log=$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/test-watchdog.XXXXXX")
started=$(date +%s)

# A process group of its own, so that what the command started (test binaries,
# the runtimes they start) ends with it.
set -m
"$@" >"$log" 2>&1 &
pid=$!
set +m

# What the job's console shows, as it comes.
tail -n +1 -f "$log" &
tailer=$!

cleanup() {
  kill "$tailer" 2>/dev/null
  wait "$tailer" 2>/dev/null
}
trap cleanup EXIT

stop() {
  # $1: why. Ends the command, prints what it was doing, and returns 124.
  echo "::error::test-watchdog: $1"
  echo "test-watchdog: the last lines before it stopped:"
  tail -n 40 "$log"
  kill -TERM -"$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null
  for _ in $(seq 1 10); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 1
  done
  kill -KILL -"$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null
  rm -f "$log"
  exit 124
}

last_size=-1
last_change=$started
while kill -0 "$pid" 2>/dev/null; do
  size=$(wc -c <"$log" 2>/dev/null || echo 0)
  now=$(date +%s)
  if [ "$size" != "$last_size" ]; then
    last_size=$size
    last_change=$now
  fi
  if [ $((now - last_change)) -ge "$stall" ]; then
    stop "no output for ${stall}s (a test is stuck; libtest names it above)"
  fi
  if [ "$limit" -gt 0 ] && [ $((now - started)) -ge "$limit" ]; then
    stop "still running after ${limit}s"
  fi
  sleep 2
done

wait "$pid"
status=$?
rm -f "$log"
exit "$status"

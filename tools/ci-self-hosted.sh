#!/usr/bin/env bash
# The self-hosted runners keep their state between jobs, and several runner
# instances share one machine (docs/ci.md §10): on macOS in one user's home,
# on Linux each in its own container. The jobs that run on them call this at
# their start and, with `if: always()`, at their end.
#
#   start  ends what an earlier job of this runner left running, empties the
#          scenario directory, and points the job at this runner's own
#          directories (GITHUB_ENV): RUTIS_E2E_DIR, short for Unix socket
#          paths (tests/e2e/src/lib.rs; /tmp/rutis-e2e for a runner alone),
#          and CARGO_TARGET_DIR, kept between jobs in place of rust-cache
#          (the job's own with RUTIS_CI_FRESH_TARGET=1),
#          emptied above RUTIS_CI_TARGET_MAX_GB; and limits CARGO_BUILD_JOBS
#          and RUST_TEST_THREADS to the machine's share of this instance
#          (RUTIS_CI_JOBS overrides it); GOCACHE, this instance's own;
#          RUTIS_TEST_TIME_SCALE when the workflow passes one (the Linux
#          runners: their containers share a loaded host), which lengthens
#          the tests' hang guards (crates/rutis-test-support)
#   end    ends what this job left running and empties the scenario
#          directory; after a failed job (JOB_STATUS=failure), empties the
#          instance's Go build cache
#
# Everything is scoped to this runner instance (RUNNER_NAME): its workspace,
# target directory and scenario directory. Processes of another instance's
# job, and the runner itself, are left alone.
#
# Usage: tools/ci-self-hosted.sh start|end
set -uo pipefail

phase=${1:?usage: $0 start|end}
: "${RUNNER_NAME:?}" "${GITHUB_WORKSPACE:?}"
cache="$HOME/.cache/rutis-ci/$RUNNER_NAME"
target="$cache/target"
# Jobs that build the dylib SDK (RUTIS_CI_FRESH_TARGET=1) build in a target
# directory of their own job instead, which the runner empties afterwards:
# a dylib's file name carries no hash, so builds of rutis-sdk with different
# features or flags write the same librutis_sdk file, and a kept directory
# came to hand rutis-cli an SDK built against another serde_json, which cargo
# took for up to date (E0308, #376). GitHub's runners start empty.
if [ "${RUTIS_CI_FRESH_TARGET:-}" = 1 ]; then
  target="${RUNNER_TEMP:?}/target"
fi
# The machine's runner instances: each a ~/actions-runner*/ with a .runner
# file. A runner alone (as in the Linux runners' containers, one each, with
# their own /tmp and long names) takes the default, shortest directory.
instances=$(ls -d "$HOME"/actions-runner*/.runner 2>/dev/null | wc -l | tr -d ' ')
[ "$instances" -ge 1 ] 2>/dev/null || instances=1
if [ "$instances" -gt 1 ]; then
  e2e="/tmp/rutis-e2e-$RUNNER_NAME"
else
  e2e=/tmp/rutis-e2e
fi

# This step's shell and its ancestors (the runner's worker among them).
keep=" $$ "
pid=$$
while [ "$pid" -gt 1 ]; do
  pid=$(ps -o ppid= -p "$pid" | tr -d ' ')
  [ -n "$pid" ] || break
  keep="$keep$pid "
done

# This user's processes whose command line or working directory is under
# this runner's directories.
victims() {
  {
    ps -U "$(id -u)" -ww -o pid=,args= | while read -r pid args; do
      for dir in "$GITHUB_WORKSPACE" "$target" "$e2e" "/private$e2e"; do
        case "$args/" in *"$dir"/*) echo "$pid"; break ;; esac
      done
    done
    # The working directories: lsof on macOS, /proc on Linux (whose
    # runner containers have no lsof).
    if [ -d /proc/self/cwd ]; then
      for link in /proc/[0-9]*/cwd; do
        [ -O "$link" ] || continue
        cwd=$(readlink "$link" 2>/dev/null) || continue
        pid=${link#/proc/}; pid=${pid%/cwd}
        echo "p$pid"; echo "n$cwd"
      done
    else
      lsof -a -u "$(id -u)" -d cwd -Fn 2>/dev/null
    fi | while read -r line; do
      case "$line" in
        p*) pid=${line#p} ;;
        n*) for dir in "$GITHUB_WORKSPACE" "$target" "$e2e" "/private$e2e"; do
              case "${line#n}/" in "$dir"/*) echo "$pid"; break ;; esac
            done ;;
      esac
    done
  } | sort -un | not_ours
}

# Drops this step's shell, its ancestors and its descendants (the pipelines
# listing the processes), and the processes already gone.
not_ours() {
  local mine=" $$ " grew=1 pid ppid table
  table=$(ps -A -o pid=,ppid=)
  while [ -n "$grew" ]; do
    grew=
    while read -r pid ppid; do
      case "$mine" in
        *" $pid "*) ;;
        *) case "$mine" in *" $ppid "*) [ "$ppid" != 1 ] && { mine="$mine$pid "; grew=1; } ;; esac ;;
      esac
    done <<< "$table"
  done
  while read -r pid; do
    case "$mine$keep" in *" $pid "*) continue ;; esac
    kill -0 "$pid" 2>/dev/null && echo "$pid"
  done
  return 0
}

stop_leftovers() {
  local pids
  pids=$(victims | tr '\n' ' ')
  [ -n "${pids// /}" ] || return 0
  echo "ending leftover processes: $pids"
  ps -ww -o pid=,args= -p "$(echo $pids | tr ' ' ,)" 2>/dev/null | cut -c1-200 || true
  kill -TERM $pids 2>/dev/null || true
  sleep 2
  pids=$(victims | tr '\n' ' ')
  [ -n "${pids// /}" ] && kill -KILL $pids 2>/dev/null
  return 0
}

remove() {
  [ -e "$1" ] || return 0
  chmod -R u+w "$1" 2>/dev/null
  rm -rf "$1"
}

stop_leftovers
remove "$e2e"

if [ "$phase" = start ]; then
  # The tools/*.sh scripts leave their /tmp/rutis-* directories, and a job
  # cut short leaves anything; no job runs 12 hours (6 at most), so older
  # ones are no running job's.
  find /tmp/ -maxdepth 1 -name 'rutis-*' -user "$(id -un)" -mmin +720 -print 2>/dev/null |
    while IFS= read -r dir; do remove "$dir"; done
  # A machine whose cargo is mbx's shim (a build cache; its directory holds
  # mbx-target) keeps the builds in mbx's own target directories: a target
  # directory of ours, filled by mbx and by cargo alike, came to hold
  # rutis-sdk and rutis-cli built against two different serde_json (E0308).
  cargo_bin=$(command -v cargo 2>/dev/null || true)
  if [ "${RUTIS_CI_FRESH_TARGET:-}" = 1 ]; then
    echo "a target directory of this job's own: $target"
    mkdir -p "$target"
  elif [ -n "$cargo_bin" ] && [ -f "$(dirname "$cargo_bin")/mbx-target" ]; then
    echo "cargo is mbx ($cargo_bin): mbx keeps the target directory"
    remove "$target"
    target=
  else
    mkdir -p "$target"
    used=$(du -sk "$target" | cut -f 1)
    max=${RUTIS_CI_TARGET_MAX_GB:-30}
    if [ "$used" -gt $((max * 1024 * 1024)) ]; then
      echo "target directory is $((used / 1024 / 1024)) GB (over $max GB): emptying it"
      remove "$target"
      mkdir -p "$target"
    fi
  fi
  mkdir -p "$cache"
  df -h "$cache" | tail -n 1
  echo "open files limit: $(ulimit -Sn) (hard $(ulimit -Hn))"
  # The machine's CPUs shared by its runner instances, which may all build
  # at once: cargo's and the test binaries' parallelism, RUTIS_CI_JOBS if
  # set (the Linux runners' containers see the machine's CPUs but not each
  # other: the workflow passes LINUX_CI_JOBS there).
  cpus=$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu)
  jobs=${RUTIS_CI_JOBS:-$((cpus / instances))}
  [ "$jobs" -ge 2 ] 2>/dev/null || jobs=2
  echo "load: $(uptime | sed 's/.*load averages*: //'); $cpus CPUs, $instances runner instances: $jobs jobs and test threads"
  {
    echo "RUTIS_E2E_DIR=$e2e"
    [ -z "$target" ] || echo "CARGO_TARGET_DIR=$target"
    echo "CARGO_BUILD_JOBS=$jobs"
    echo "RUST_TEST_THREADS=$jobs"
    # A GOROOT of the machine's own Go (the Linux runners' image sets one)
    # would pair setup-go's go command with another release's compiler.
    # Empty, go finds its own.
    [ -z "${GOROOT:-}" ] || echo "GOROOT="
    # Go's build cache, this instance's own: the runner instances of a
    # machine would otherwise share one, and emptying it (below) would pull
    # files from under another instance's build.
    echo "GOCACHE=$cache/go-build"
    # The tests' hang guards, lengthened on a loaded machine; a real hang
    # still fails, only later (docs/ci.md §11).
    [ -z "${RUTIS_TEST_TIME_SCALE:-}" ] || echo "RUTIS_TEST_TIME_SCALE=$RUTIS_TEST_TIME_SCALE"
  } >> "$GITHUB_ENV"
  [ -z "${RUTIS_TEST_TIME_SCALE:-}" ] || echo "test hang guards scaled by $RUTIS_TEST_TIME_SCALE"
fi

# A job that failed may have been one cut short while Go wrote its build
# cache, which then holds a truncated entry and fails every later build that
# reads it ("malformed elf file", #374). After a failure the cache goes; the
# next job builds it again (about a minute). JOB_STATUS is the workflow's
# `job.status`.
if [ "$phase" = end ] && [ "${JOB_STATUS:-}" = failure ] && [ -d "$cache/go-build" ]; then
  echo "the job failed: emptying this instance's Go build cache ($cache/go-build)"
  remove "$cache/go-build"
fi

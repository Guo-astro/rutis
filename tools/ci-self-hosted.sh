#!/usr/bin/env bash
# The self-hosted macOS runners keep their state between jobs, and several
# runner instances share one machine (docs/ci.md §10). The jobs that run on
# them call this at their start and, with `if: always()`, at their end.
#
#   start  ends what an earlier job of this runner left running, empties the
#          scenario directory, and points the job at this runner's own
#          directories (GITHUB_ENV): RUTIS_E2E_DIR, short for Unix socket
#          paths (tests/e2e/src/lib.rs), and CARGO_TARGET_DIR, kept between
#          jobs in place of rust-cache, emptied above RUTIS_CI_TARGET_MAX_GB
#   end    ends what this job left running and empties the scenario directory
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
e2e="/tmp/rutis-e2e-$RUNNER_NAME"

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
    lsof -a -u "$(id -u)" -d cwd -Fn 2>/dev/null | while read -r line; do
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
  mkdir -p "$target"
  used=$(du -sk "$target" | cut -f 1)
  max=${RUTIS_CI_TARGET_MAX_GB:-30}
  if [ "$used" -gt $((max * 1024 * 1024)) ]; then
    echo "target directory is $((used / 1024 / 1024)) GB (over $max GB): emptying it"
    remove "$target"
    mkdir -p "$target"
  fi
  df -h "$cache" | tail -n 1
  echo "open files limit: $(ulimit -Sn) (hard $(ulimit -Hn))"
  {
    echo "RUTIS_E2E_DIR=$e2e"
    echo "CARGO_TARGET_DIR=$target"
  } >> "$GITHUB_ENV"
fi

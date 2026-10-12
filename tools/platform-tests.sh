#!/usr/bin/env bash
# The test targets of a crate that pull requests run on macOS and Windows:
# the unit tests (--lib) and every integration test file but those named
# (the PURE_TESTS_* lists of .github/workflows/ci.yml). Prints the cargo test
# flags on one line. A named file that does not exist is an error, so a
# renamed or removed test file cannot leave a stale name in a list.
#
# Usage: tools/platform-tests.sh <crate directory> [<test name>...]
#   tools/platform-tests.sh crates/rutis-bridge link multihop
#   → --lib --test boundary_rules --test bun_channel ...
set -euo pipefail

[ $# -ge 1 ] || { echo "usage: $0 <crate directory> [<test name>...]" >&2; exit 2; }
dir=$1
shift
[ -d "$dir/tests" ] || { echo "::error::$dir/tests is not a directory" >&2; exit 1; }

# The integration test targets cargo finds on its own: tests/<name>.rs and
# tests/<name>/main.rs.
shopt -s nullglob
names=()
for file in "$dir"/tests/*.rs "$dir"/tests/*/main.rs; do
  case $file in
    */main.rs) name=${file%/main.rs}; name=${name##*/} ;;
    *) name=${file##*/}; name=${name%.rs} ;;
  esac
  names+=("$name")
done

status=0
for excluded in "$@"; do
  found=
  for name in "${names[@]}"; do
    [ "$name" = "$excluded" ] && found=1
  done
  [ -n "$found" ] || { echo "::error::$dir: no test file named $excluded (PURE_TESTS_* in ci.yml)" >&2; status=1; }
done
[ "$status" -eq 0 ] || exit 1

flags=(--lib)
for name in "${names[@]}"; do
  for excluded in "$@"; do
    [ "$name" = "$excluded" ] && continue 2
  done
  flags+=(--test "$name")
done
echo "${flags[*]}"

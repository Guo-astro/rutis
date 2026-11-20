#!/usr/bin/env bash
# Run TLC on the configurations of this directory and compare each outcome
# with expected.txt. Every `call-*.cfg` checks CallLifecycle.tla and every
# `relay-*.cfg` CallRelay.tla. An outcome is `ok`, `deadlock` (TLC's deadlock
# check), or `violated:<name>` for an invariant or the liveness property
# (a temporal property is reported as `violated:liveness`).
#   TLA2TOOLS=/path/to/tla2tools.jar ./run.sh [--mutations | name ...]
# By default, skips `*-mut-*.cfg` mutation configs (pass `--mutations` or `--all`
# to run them when editing the model).
# Needs Java 11+ on PATH (for example `mise exec java@temurin-21 -- ./run.sh`).
# tla2tools.jar: https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4
# (do not commit it).
set -u
cd "$(dirname "$0")"
jar="${TLA2TOOLS:?set TLA2TOOLS to the path of tla2tools.jar}"
out="${TLC_OUT:-$(mktemp -d)}"
mkdir -p "$out"
include_mut=false
names=()
for arg in "$@"; do
  case "$arg" in
    --mutations|--all) include_mut=true ;;
    *) names+=("$arg") ;;
  esac
done
if [ ${#names[@]} -eq 0 ]; then
  for cfg in *.cfg; do
    name="${cfg%.cfg}"
    if [ "$include_mut" = false ] && [[ "$name" == *-mut-* ]]; then continue; fi
    names+=("$name")
  done
fi
status=0
for name in "${names[@]}"; do
  case "$name" in
    call-*) module=CallLifecycle.tla ;;
    relay-*) module=CallRelay.tla ;;
    *) echo "$name: no module for this name" >&2; status=1; continue ;;
  esac
  log="$out/$name.log"
  start=$(date +%s)
  java -XX:+UseParallelGC -cp "$jar" tlc2.TLC -workers "${TLC_WORKERS:-auto}" -metadir "$out/$name.states" \
    -config "$name.cfg" "$module" > "$log" 2>&1
  seconds=$(( $(date +%s) - start ))
  if inv=$(grep -oE '^Error: Invariant [A-Za-z]+ is violated' "$log"); then
    got=$(echo "$inv" | awk '{print "violated:" $3}')
  elif grep -q '^Error: Temporal properties were violated' "$log"; then
    got=violated:liveness
  elif grep -q '^Error: Deadlock reached' "$log"; then
    got=deadlock
  elif grep -q '^Model checking completed. No error has been found' "$log"; then
    got=ok
  else
    got=failed
  fi
  want=$(awk -v n="$name" '$1 == n {print $2}' expected.txt)
  states=$(grep -oE '[0-9,]+ distinct states found' "$log" | tail -1)
  printf '%-40s %-30s expected %-30s (%s; %ss; log %s)\n' "$name" "$got" "${want:-?}" "$states" "$seconds" "$log"
  [ "$got" = "$want" ] || status=1
done
exit $status

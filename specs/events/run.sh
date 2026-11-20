#!/usr/bin/env bash
# Run TLC on the configurations of this directory and compare each outcome
# with expected.txt. `host-defs` checks EventsHostCheck.tla (the definitions
# of EventsHost.tla); `table-*`, `emit-*` and `link-*` check their modules.
#   TLA2TOOLS=/path/to/tla2tools.jar ./run.sh [--mutations | name ...]
# By default, skips `*-mut-*.cfg` mutation configs (pass `--mutations` or `--all`
# to run them when editing the model).
# Configurations whose .cfg says CHECK_DEADLOCK TRUE rely on the Done action:
# it stutters only when the protocol has no work left, so a stuck state with
# work remaining is still reported as a deadlock.
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
    host-defs) module=EventsHostCheck.tla ;;
    table-*) module=EventsTable.tla ;;
    emit-*) module=EventsEmit.tla ;;
    link-*) module=EventsLink.tla ;;
    *) echo "$name: no module for this name" >&2; status=1; continue ;;
  esac
  log="$out/$name.log"
  start=$(date +%s)
  java -XX:+UseParallelGC -cp "$jar" tlc2.TLC -workers "${TLC_WORKERS:-auto}" \
    -metadir "$out/$name.states" -config "$name.cfg" "$module" > "$log" 2>&1
  seconds=$(( $(date +%s) - start ))
  if grep -q '^Error: Deadlock reached' "$log"; then
    got=deadlock
  elif inv=$(grep -oE '^Error: Invariant [A-Za-z]+ is violated' "$log"); then
    got=$(echo "$inv" | awk '{print "violated:" $3}')
  elif grep -q '^Error: Temporal properties were violated' "$log"; then
    got=violated:temporal
  elif grep -q '^Model checking completed. No error has been found' "$log"; then
    got=ok
  else
    got=failed
  fi
  want=$(awk -v n="$name" '$1 == n {print $2}' expected.txt)
  states=$(grep -oE '[0-9,]+ distinct states found' "$log" | tail -1)
  printf '%-30s %-32s expected %-32s (%s; %ss; log %s)\n' \
    "$name" "$got" "${want:-?}" "${states:-no states}" "$seconds" "$log"
  [ "$got" = "$want" ] || status=1
done
exit $status

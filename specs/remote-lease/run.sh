#!/usr/bin/env bash
# Run TLC on the configurations of this directory and compare each outcome
# with expected.txt. `lease-*.cfg` check RemoteLease.tla, `access-*.cfg`
# AccessPoint.tla. Liveness configurations (`lease-healthy`,
# `lease-faults-live`, `lease-app-restart`, `access-healthy-*`,
# `access-faults-live`, `access-static-revoke`) need weak fairness; the
# others check safety only. The #615 configurations: `lease-takeover`
# (authentication before the close), `lease-cleanup-deadline` (a cleanup
# deadline), `lease-app-restart` / `lease-app-restart-3` (stop then start,
# once and three times), `lease-stop-during-takeover`, `lease-faults-live`
# (liveness after transient faults), `access-app-restart`,
# `access-faults-live`, and `access-static-revoke` (a revoked static listen
# link, a revocation during the handshake included).
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
    lease-*) module=RemoteLease.tla ;;
    access-*) module=AccessPoint.tla ;;
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
  elif grep -q '^Model checking completed. No error has been found' "$log"; then
    got=ok
  else
    got=failed
  fi
  want=$(awk -v n="$name" '$1 == n {print $2}' expected.txt)
  states=$(grep -oE '[0-9,]+ distinct states found' "$log" | tail -1)
  printf '%-34s %-30s expected %-30s (%s; %ss; log %s)\n' "$name" "$got" "${want:-?}" "$states" "$seconds" "$log"
  [ "$got" = "$want" ] || status=1
done
exit $status

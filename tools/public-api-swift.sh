#!/usr/bin/env bash
# The public API of the Swift SDK (the libraries `Rutis` and `RutisTest`),
# one declaration a line as the symbol graph lists it, against the snapshot
# in swift/rutis/public-api.txt (SDK entry design §6, #692). The deprecated
# names (moved to `Rutis.Advanced` / `Rutis._Generated` in 0.19) are in a
# section of their own. A new name changes the snapshot, so a review sees it.
#
# Usage: tools/public-api-swift.sh          compare; exit 1 on a difference
#        tools/public-api-swift.sh --write  update the snapshot
# Needs macOS (SwiftPM and jq, which macOS 15 ships).
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
snapshot="$repo_dir/swift/rutis/public-api.txt"
current="$(mktemp)"
trap 'rm -f "$current"' EXIT
cd "$repo_dir"
# The command says where it wrote the graphs; the place differs between
# SwiftPM's build systems, so it is read, not worked out.
written="$(swift package dump-symbol-graph --minimum-access-level public --skip-synthesized-members | sed -n 's/^Files written to //p' | tail -n 1)"
graphs="${written:-$(swift build --show-bin-path)/../symbolgraph}"
# One line per symbol: its path, then its declaration. Members the compiler
# synthesizes are left out; `deprecated` marks the moved names.
lines() {
  jq -r --arg deprecated "$2" '
    .symbols[]
    | select((.identifier.precise | contains("::SYNTHESIZED::")) | not)
    | select(([.availability[]? | select(.isUnconditionallyDeprecated == true)] | length > 0) == ($deprecated == "yes"))
    | (.pathComponents | join(".")) + "  " + ([.declarationFragments[].spelling] | join("") | gsub("\n"; " "))
  ' "$graphs/$1.symbols.json" | LC_ALL=C sort
}
for module in Rutis RutisTest; do
  echo "## $module"
  lines "$module" no
  deprecated="$(lines "$module" yes)"
  if [ -n "$deprecated" ]; then
    echo "## $module (deprecated: moved to Rutis.Advanced or Rutis._Generated in 0.19)"
    echo "$deprecated"
  fi
  echo
done > "$current"
if [ "${1:-}" = "--write" ]; then
  cp "$current" "$snapshot"
  echo "wrote $snapshot"
  exit 0
fi
if ! diff -u "$snapshot" "$current"; then
  echo "::error::the public API of the Swift SDK changed: review the diff above, then run tools/public-api-swift.sh --write" >&2
  exit 1
fi
echo "public API unchanged"

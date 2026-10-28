#!/usr/bin/env bash
# The public API of the Swift SDK (the libraries `Rutis` and `RutisTest`),
# one symbol a line (its path and kind) as the symbol graph lists it,
# against the snapshot in swift/rutis/public-api.txt (SDK entry design §6,
# #692). A new name changes the snapshot, so a review sees it.
#
# Usage: tools/public-api-swift.sh          compare; exit 1 on a difference
#        tools/public-api-swift.sh --write  update the snapshot
# Needs macOS (SwiftPM and jq, which macOS 15 ships).
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
snapshot="$repo_dir/swift/rutis/public-api.txt"
cd "$repo_dir"
# The command says where it wrote the graphs; the place differs between
# SwiftPM's build systems, so it is read, not worked out.
written="$(swift package dump-symbol-graph --minimum-access-level public --skip-synthesized-members | sed -n 's/^Files written to //p' | tail -n 1)"
graphs="${written:-$(swift build --show-bin-path)/../symbolgraph}"
# One line per symbol: its path (which holds a function's argument labels),
# then its kind. Not its declaration: compilers spell those differently
# (Swift 6.3 writes `any Encoder`, 6.4 `Encoder`). Members the compiler
# synthesizes are left out.
lines() {
  jq -r '
    .symbols[]
    | select((.identifier.precise | contains("::SYNTHESIZED::")) | not)
    | (.pathComponents | join(".")) + "  " + .kind.identifier
  ' "$graphs/$1.symbols.json" | LC_ALL=C sort
}
public_api() {
  for module in Rutis RutisTest; do
    echo "## $module"
    lines "$module"
    echo
  done
}
. "$repo_dir/tools/public-api-check.sh"
public_api_check "$snapshot" "the Swift SDK" "$@"

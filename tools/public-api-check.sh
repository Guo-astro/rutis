# The comparison both public API scripts share (tools/public-api.sh,
# tools/public-api-swift.sh), sourced by them. The script defines
# `public_api`, which prints the current public API, then calls
#
#   public_api_check SNAPSHOT WHAT "$@"
#
# With --write it replaces SNAPSHOT; otherwise it compares and exits 1 on a
# difference, naming WHAT ("rutis", "the Swift SDK") in the error.
public_api_check() {
  local snapshot="$1" what="$2" mode="${3:-}"
  local script="tools/$(basename "$0")"
  current="$(mktemp)"
  trap 'rm -f "$current"' EXIT
  public_api > "$current"
  if [ "$mode" = "--write" ]; then
    cp "$current" "$snapshot"
    echo "wrote $snapshot"
    exit 0
  fi
  if ! diff -u "$snapshot" "$current"; then
    echo "::error::the public API of $what changed: review the diff above, then run $script --write" >&2
    exit 1
  fi
  echo "public API unchanged"
}

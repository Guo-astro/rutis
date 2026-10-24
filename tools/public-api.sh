#!/usr/bin/env bash
# The public API of the rutis kernel (its root, `rutis::events` and
# `rutis::host`; the hidden `rutis::internal` is left out), as
# cargo-public-api lists it, against the snapshot in
# crates/rutis/public-api.txt (design-rust-api-convergence §5, #617). A new
# name changes the snapshot, so a review sees it.
#
# Usage: tools/public-api.sh          compare; exit 1 on a difference
#        tools/public-api.sh --write  update the snapshot
# Needs cargo-public-api and the nightly toolchain it reads rustdoc JSON with
# (RUTIS_PUBLIC_API_TOOLCHAIN, default `nightly`).
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
snapshot="$repo_dir/crates/rutis/public-api.txt"
toolchain="${RUTIS_PUBLIC_API_TOOLCHAIN:-nightly}"
current="$(mktemp)"
trap 'rm -f "$current"' EXIT
(cd "$repo_dir" && cargo +"$toolchain" public-api -p rutis --simplified) > "$current"
if [ "${1:-}" = "--write" ]; then
  cp "$current" "$snapshot"
  echo "wrote $snapshot"
  exit 0
fi
if ! diff -u "$snapshot" "$current"; then
  echo "::error::the public API of rutis changed: review the diff above, then run tools/public-api.sh --write" >&2
  exit 1
fi
echo "public API unchanged"

#!/usr/bin/env bash
# The public API of the rutis kernel (its root, `rutis::events` and
# `rutis::host`; the hidden `rutis::internal` is left out), as
# `cargo public-api --simplified` lists it, against the snapshot in
# crates/rutis/public-api.txt (design-rust-api-convergence §5, #617). A new
# name changes the snapshot, so a review sees it.
#
# Usage: tools/public-api.sh          compare; exit 1 on a difference
#        tools/public-api.sh --write  update the snapshot
# Needs the nightly toolchain the rustdoc JSON is read with
# (RUTIS_PUBLIC_API_TOOLCHAIN, default `nightly`). The listing is
# tools/public-api, built here: the cargo-public-api libraries without the
# CLI, which needs OpenSSL to build.
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
snapshot="$repo_dir/crates/rutis/public-api.txt"
toolchain="${RUTIS_PUBLIC_API_TOOLCHAIN:-nightly}"
current="$(mktemp)"
trap 'rm -f "$current"' EXIT
cargo run --quiet --locked \
  --manifest-path "$repo_dir/tools/public-api/Cargo.toml" \
  --target-dir "$repo_dir/target/public-api-tool" \
  -- "$toolchain" "$repo_dir/Cargo.toml" rutis > "$current"
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

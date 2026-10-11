#!/usr/bin/env bash
# Build this machine's packages the way package.yml does, into <dir>, for the
# install scenarios (tests/e2e/tests/install.rs) on a development machine:
#
#   tools/install/local-artifacts.sh <dir>
#   RUTIS_E2E_ARTIFACTS=<dir> cargo test -p rutis-e2e --test install -- --ignored --test-threads=1
#
# <dir>/npm     the npm tarballs: @arcships/rutis, -runtime, -bun, -host and
#               this platform's @arcships/rutis-host-<os>-<cpu>
# <dir>/wheels  rutis (pure Python) and this platform's rutis-host wheel
# <dir>/binary  rutis-host-<version>-<target>.tar.gz (.zip on Windows) and
#               its .sha256
set -euo pipefail

out=${1:?usage: tools/install/local-artifacts.sh <dir>}
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
mkdir -p "$out"
out=$(cd "$out" && pwd)
rm -rf "$out/npm" "$out/wheels" "$out/binary"
mkdir -p "$out/npm" "$out/wheels" "$out/binary"

version=$(node -p "require('./node/rutis-host/package.json').version")
platform=$(node -p "process.platform + '-' + process.arch")
target=$(rustc -vV | sed -n 's/^host: //p')
exe=""
[[ "$platform" == win32-* ]] && exe=".exe"

cargo build --locked --release -p rutis-host
binary="target/release/rutis-host$exe"

# npm: the platform package is made from the binary, like release.yml does.
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
node node/rutis-host/scripts/platform-package.mjs "${platform%-*}" "${platform##*-}" "$binary" "$stage/platform" >/dev/null
(cd "$stage/platform" && npm pack --silent --pack-destination "$out/npm" >/dev/null)
for package in node/rutis node/rutis-runtime bun/rutis-bun node/rutis-host; do
  (cd "$package" && npm pack --silent --pack-destination "$out/npm" >/dev/null)
done

# PyPI
uv build --quiet python/rutis --wheel --out-dir "$out/wheels"
uvx --quiet maturin build --quiet --release --locked --manifest-path crates/rutis-host/Cargo.toml --out "$out/wheels"

# The release archive
name="rutis-host-$version-$target"
mkdir -p "$stage/$name"
cp "$binary" crates/rutis-host/README.md LICENSE "$stage/$name/"
if [[ -n "$exe" ]]; then
  archive="$name.zip"
  (cd "$stage" && 7z a -tzip "$out/binary/$archive" "$name" >/dev/null)
else
  archive="$name.tar.gz"
  tar -C "$stage" -czf "$out/binary/$archive" "$name"
fi
(cd "$out/binary" && { sha256sum "$archive" 2>/dev/null || shasum -a 256 "$archive"; } > "$archive.sha256")

echo "$out: rutis $version for $platform ($target)"
ls "$out"/npm "$out"/wheels "$out"/binary

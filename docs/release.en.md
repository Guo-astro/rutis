# Releases

## Release train

Except for `rutis-cli` (`cli-v*`, `release-cli.yml`), these packages share a version and are released together (from 0.8.0, the core and the dylib toolchain included):

| Registry | Packages |
| --- | --- |
| crates.io | `rutis` (the core), `rutis-bridge`, `rutis-loader`, `rutis-host`; the dylib toolchain `rutis-sdk`, `rutis-dylib`, `rutis-dylib-meta`, `rutis-dylib-launcher` |
| npm | `@arcships/rutis`, `@arcships/rutis-runtime`, `@arcships/rutis-bun`, `@arcships/rutis-host` and `@arcships/rutis-host-{linux,darwin}-{x64,arm64}`, `@arcships/rutis-host-win32-x64` |
| PyPI | `rutis`, `rutis-host` (wheels for each platform) |
| Go | `github.com/arcships/rutis/go/rutis` (the tag `go/rutis/vX.Y.Z`, pushed by release.yml) |
| GitHub Release | `rutis-host` binaries |

## Steps

1. Update versions in the Cargo.toml of every crate listed in `crates` in `scripts/train.mjs` (including the dependency versions between them and from other workspace crates); `node/rutis`, `node/rutis-runtime`, `bun/rutis-bun`, and `node/rutis-host` package.json files (including the runtime and platform package versions used by `@arcships/rutis-host`); `python/rutis/pyproject.toml` and `IMPLEMENTATION` in `python/rutis/rutis/peer.py`; `Version` in `go/rutis/version.go`; and the `rutis` range in `crates/rutis-host/pyproject.toml`. `node scripts/train.mjs` checks that they match, and CI runs it too. In the same PR, save this version's persisted samples (`tests/fixtures/persisted/`; see [Persisted samples](#persisted-samples) below).
2. Merge to `main` and confirm the CI `release-dry-run` passes (all packages can be built) and main's install.yml passes (this build's packages installed and run on three platforms).
3. Run the network smoke test on two machines before publishing (see below).
4. Create and push a tag such as `vX.Y.Z`. `release.yml` checks versions, builds the binaries, wheels and npm packages for five platforms (package.yml, the same build steps as main) → **release gate**: installs and runs those files on all five platforms (install.yml); if one fails, nothing is published → semver-checks the crates already on crates.io, publishes the crates in dependency order (`node scripts/train.mjs --crates`), then the npm and PyPI packages (exactly the files the gate installed) → creates a GitHub Release (its notes come from `docs/releases/X.Y.Z.en.md`, which must exist before tagging) → **after publishing**: waits until the registries serve the version, then on Linux installs and runs the npm, PyPI and binary channels from them, `cargo install` and the Go module, and follows the guides' marked examples. Each step skips versions already present in its registry, so after fixing a mid-release failure you can rerun it.
5. When the check after publishing fails, release.yml opens an issue (assigned to the maintainer). Read the run's logs to tell a broken package from a problem of the check or a registry. For a broken package: on npm `npm deprecate @arcships/<package>@X.Y.Z "<reason>"`, on PyPI yank the version on the project page, on crates.io `cargo yank --version X.Y.Z <crate>`; then release the fix as X.Y.(Z+1). Published files are not withdrawn.

Required configuration: GitHub environment `release` with `CARGO_TOKEN` and `NPM_TOKEN`, and environments `pypi` and `pypi-host`. On PyPI, the trusted publisher of `rutis` points to `release.yml` with environment `pypi`, and that of `rutis-host` to environment `pypi-host`: the two must differ, or a release gets a token valid for only one of them.

Increment `PLUGIN_API` (present in both the SDK and runtime) only when the interface visible to plugins becomes incompatible. Increment the session protocol version (`rutisProtocol` and `rutis_bridge::session::PROTOCOL`) when the wire format becomes incompatible.

## Persisted samples

`tests/fixtures/persisted/<version>/` keeps the persisted data each released version wrote. The tests read every sample with the current code, and the result must be what that version read (risk J3):

| File | Content | Test |
| --- | --- | --- |
| `rutis.json` | A configuration using as many of that version's fields as possible | `cargo test -p rutis-host --test persisted` |
| `rutis.expected.json` | What that version read from it (normalized; paths under the sample's directory written as `<dir>`) | Same |
| `cordis.patch.yml` | A dsh user layer using as much of that version's format as possible | `cargo test -p rutis-dsh --test persisted` |
| `cordis.patch.expected.json` | The patch list that version read from it | Same |

When releasing:

1. Copy the previous version's directory to `tests/fixtures/persisted/X.Y.Z/`. If this version added fields or forms to `rutis.json` or the user layer, use them in the samples.
2. Regenerate this version's expectations with this version's code: `RUTIS_PERSISTED_WRITE=X.Y.Z cargo test -p rutis-host --test persisted` and `RUTIS_PERSISTED_WRITE=X.Y.Z cargo test -p rutis-dsh --test persisted` (only the expectation files under `X.Y.Z/` are written). Review the diff, then run both again without the variable.
3. Never change earlier versions' samples or expectations. An earlier sample that no longer reads as it did is an incompatible change ([quality standard](quality-standard.md) §9.3). When a format is no longer supported, move its sample to `rejected/<name>/` and put the error in `rutis.error` next to it; the error must say how to migrate.

`rutis.json` has a format version (top-level `format`, currently 1): when its meaning changes, increase `FORMAT` (`crates/rutis-host/src/config.rs`) and say so in the release notes. dsh's `cordis.patch.yml` is a Cordis configuration format and has no version; its compatibility rests on these samples alone.

## Smoke test

```text
# Listener (the certificate must match its hostname)
cargo run -p rutis-bridge --features websocket --example smoke -- \
    listen 0.0.0.0:7443 --cert server.pem --key server.key --token secret

# Dialer
cargo run -p rutis-bridge --features websocket --example smoke -- \
    dial wss://<listener-hostname>:7443/rutis --ca ca.pem --token secret
```

Expected behavior: the dialer prints `clock: <n>` every second. After a network outage, both sides report a heartbeat timeout and wait to reconnect within 30 seconds; after recovery, they report `ready, session <n+1>`. Restarting the listener makes the dialer retry with backoff. A wrong token returns `AuthRejected … 403`; an untrusted CA returns `AuthRejected … UnknownIssuer`.

[Write a TypeScript plugin](guide/typescript-plugin.en.md) and [Write a Python plugin](guide/python-plugin.en.md) are both followed by the check after publishing (their marked examples, `tools/doc-examples.mjs`).

The nightly stress workflow also runs two soak tests (repeated link disconnect/reconnect and repeated process start/exit) and checks that file descriptor and thread counts do not grow and all processes are reaped.

## 0.10.0

The first release that saves its persisted samples in the release PR (the samples of 0.8.0–0.9.1 were added afterwards in #284), and the first in which `rutis.json` has a top-level `format`: an older file without `format` reads as 1, so the 0.10.0 expectations equal 0.9.1's and only the sample gains `"format": 1`. From this release the examples of [Write a Python plugin](guide/python-plugin.en.md) are followed by the check after publishing (#259) instead of by hand.

## 0.9.0

The Bun runtime `@arcships/rutis-bun` and the Go module `go/rutis` are released for the first time. `@arcships/rutis-bun` is a new name on npm: `NPM_TOKEN` must be able to publish new packages under `@arcships`. The Go module is released by the tag `go/rutis/v0.9.0`, which release.yml pushes and the Go module proxy serves from. Until then, projects made by `rutis-host new --lang bun|go` cannot install their dependencies. It is also the first release through the release gate and the check after publishing (#254): the linux-arm64 and darwin-x64 install cells have not run in CI before the tag.

## 0.8.0

From this release the core and the dylib toolchain are in the train: push only `v0.8.0`; the `rutis-v*` tags and publish-rutis.yml are retired. `rutis-sdk`, `rutis-dylib`, `rutis-dylib-meta` and `rutis-dylib-launcher` are published to crates.io for the first time.

## 0.7.0

0.7.0 is the train's first release and builds on core 0.6.1: push only `v0.7.0`, no `rutis-v*`. Apart from `rutis-loader` (0.1.0 before), every package is a new name on its registry: configure pending publishers for `rutis` and `rutis-host` on PyPI first. After the release succeeds, withdraw the old `rutis-interop` from crates.io and npm.

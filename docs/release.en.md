# Releases

## Release train

Except for `rutis-cli` (`cli-v*`, `release-cli.yml`), these packages share a version and are released together (from 0.8.0, the core and the dylib toolchain included):

| Registry | Packages |
| --- | --- |
| crates.io | `rutis` (the core) and its macros `rutis-macros`, `rutis-bridge`, `rutis-loader`, `rutis-host`, the type generator `rutis-types`; the dylib toolchain `rutis-sdk`, `rutis-dylib`, `rutis-dylib-meta`, `rutis-dylib-launcher` |
| npm | `@arcships/rutis`, `@arcships/rutis-runtime`, `@arcships/rutis-bun`, `@arcships/rutis-host` and `@arcships/rutis-host-{linux,darwin}-{x64,arm64}`, `@arcships/rutis-host-win32-x64` |
| PyPI | `rutis`, `rutis-host` (wheels for each platform) |
| Go | `github.com/arcships/rutis/go/rutis` (the tag `go/rutis/vX.Y.Z`, pushed by release.yml) |
| Maven Central | `io.github.arcships:rutis` (Kotlin Multiplatform: jvm, android), `io.github.arcships:rutis-ksp`, `io.github.arcships:rutis-testing` (skipped without credentials, see below) |
| GitHub Release | `rutis-host` binaries |

## Steps

1. Update versions in the Cargo.toml of every crate listed in `crates` in `scripts/train.mjs` (including the dependency versions between them and from other workspace crates); `node/rutis`, `node/rutis-runtime`, `bun/rutis-bun`, and `node/rutis-host` package.json files (including the runtime and platform package versions used by `@arcships/rutis-host`); `python/rutis/pyproject.toml` and `IMPLEMENTATION` in `python/rutis/rutis/peer.py`; `Version` in `go/rutis/internal/runtime/version.go`; `version` in `kotlin/rutis/gradle.properties` and `VERSION` in `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/advanced/Versions.kt` (the Kotlin SDK, published to Maven Central); `sdkVersion` in `swift/rutis/Sources/Rutis/Version.swift` (the Swift SDK, published by the root `vX.Y.Z` tag); and the `rutis` range in `crates/rutis-host/pyproject.toml`. `node scripts/train.mjs` checks that they match, and CI runs it too. The versions written in the guides (`README.md`, `README.zh-CN.md`, and `rust-host`, `cordis`, `typescript-plugin`, `python-plugin`, `README` and others under `docs/guide/`; search for the previous version) change in this PR too, in both languages. The lower bound Python plugins write for `rutis` does not (`rutis>=0.17` in `python-plugin` and `plugin-api`, #672): it is raised only when the plugin API changes; the templates of `rutis-host new` write the version of the rutis-host that makes the project, with nothing to edit by hand. In the same PR, save this version's persisted samples (`tests/fixtures/persisted/`; see [Persisted samples](#persisted-samples) below).
2. Merge the release PR (the CI `release-dry-run` passes on the pull request: all packages can be built). There is no need to wait for a run on main or a nightly run: after tagging, release.yml runs every CI job itself (step 5). To go through the whole release gate before tagging, run `gh workflow run release.yml --ref <branch> -f dry-run=true` on a branch (add `-f tag=vX.Y.Z` to check the version): every job of the gate runs; publishing, the GitHub Release and the checks after publishing are skipped, and no secret or environment is used.
3. Run the network smoke test on two machines before publishing (see below).
4. **An agent's trial before publishing** (once per minor version X.Y.0; it does not stop the release): have an agent follow [#663](https://github.com/arcships/rutis/issues/663)'s "evaluation method" from scratch: writing a plugin (in each language), assembling plugins with `rutis-host`, embedding in Rust, grading the 14 faults one by one, the host owner end to end. With this version's packages: those the release gate's rehearsal of step 2 built (the run's artifacts `packages`, `binary-*`, `wheels-*`), or `tools/install/local-artifacts.sh` locally; the Go SDK and the Rust crates are not on the proxy and registries yet, so from this checkout. Record each role's concepts to learn, number of commands, time, where it got stuck and where the docs disagree, against #663's concept budget; the results go in #663's evaluation records, and new problems into existing issues (a new issue only when none fits). The checks of writing a plugin, assembling plugins and a host owner are in the release gate already (`tests/e2e/tests/devx.rs`, step 5); this step judges whether it is good to use.
5. Create and push a tag such as `vX.Y.Z`. `release.yml` checks versions → **release gate**, these start together and must all pass before anything is published: every CI job (calls `ci.yml` with `all=true` and `release=true`; each job runs where `ci.yml` puts it: the Linux jobs by the repository variable `LINUX_RUNNER`, GitHub's runners while it is unset, the macOS and Windows jobs as [CI](ci.en.md) §10 and §11 say; the cross-version tests meet only releases older than this one, see below), the acceptance tests of the 14 faults in each language among them ([fault acceptance summary](faults.en.md)); the binaries, wheels and npm packages built for five platforms (package.yml, the same build steps as the nightly run), then installed and run from those files on all five platforms (install.yml; on linux-x64 the developer-experience scenarios follow: writing a plugin in each language, assembling plugins, a host owner, `tests/e2e/tests/devx.rs`, #690); the Windows dylib tests (dylib-windows.yml, with the reproducible-SDK check). If one fails, nothing is published → semver-checks the crates already on crates.io, publishes the crates in dependency order (`node scripts/train.mjs --crates`), then the npm and PyPI packages (exactly the files the gate installed; publishing and the checks after it name the official crates.io, npm, PyPI and Go module proxy explicitly, whatever mirrors a runner's configuration points at); the Go module tag `go/rutis/vX.Y.Z` also waits for the gate → creates a GitHub Release (its notes come from `docs/releases/X.Y.Z.en.md`, which must exist before tagging) → **after publishing**: waits until the registries serve the version (every npm package `node scripts/train.mjs --npm` lists: `@arcships/rutis`, `@arcships/rutis-runtime`, `@arcships/rutis-bun`, `@arcships/rutis-host` and the five platform packages its optionalDependencies name; at most 15 minutes, after which it lists the packages still missing), then on Linux installs and runs the npm, PyPI and binary channels from them, `cargo install` and the Go module, and follows the guides' marked examples. Each step skips versions already present in its registry, so after fixing a mid-release failure you can run it again (re-run the failed jobs, or `gh workflow run release.yml --ref vX.Y.Z -f dry-run=false`: `dry-run=false` runs only on a tag). A run again may find this version published in part: the gate's cross-version tests (`tests/e2e/tests/cross_version.rs`) therefore set `RUTIS_E2E_ONLY_OLDER=1` and meet only earlier releases, never this version itself.
6. When any job fails after publishing began (including a partial publish, where `go` or `github` failed and the check after publishing was skipped), release.yml opens an issue (assigned to the maintainer; when the gate fails nothing was published, no issue is opened, and the failed run says why) whose body lists each job's result. Read the run's logs to tell a broken package from a problem of the check or a registry. For a broken package: on npm `npm deprecate @arcships/<package>@X.Y.Z "<reason>"`, on PyPI yank the version on the project page, on crates.io `cargo yank --version X.Y.Z <crate>`; then release the fix as X.Y.(Z+1). Published files are not withdrawn.

Required configuration: GitHub environment `release` with `CARGO_TOKEN` and `NPM_TOKEN`, and environments `pypi` and `pypi-host`. On PyPI, the trusted publisher of `rutis` points to `release.yml` with environment `pypi`, and that of `rutis-host` to environment `pypi-host`: the two must differ, or a release gets a token valid for only one of them.

Maven Central (release.yml's `maven` job, the vanniktech maven-publish plugin) uses four secrets of the GitHub environment `maven`; when any is missing, the job says which in the run summary and skips, and the rest of the train is published as usual. The maintainer sets them up once:

1. Sign in to the [Central Portal](https://central.sonatype.com) with GitHub, add the namespace `io.github.arcships` under Namespaces, and verify it by creating, in the arcships organization, an empty public repository named with the verification key the page gives (it can be deleted afterwards).
2. Under Account → Generate User Token, generate a user token: its username and password are `MAVEN_CENTRAL_USERNAME` and `MAVEN_CENTRAL_PASSWORD`.
3. Generate a PGP key for signing and publish its public half: `gpg --quick-gen-key "arcships rutis" rsa4096 sign 0`, `gpg --keyserver keyserver.ubuntu.com --send-keys <key id>`. The whole output of `gpg --armor --export-secret-keys <key id>` is `MAVEN_SIGNING_KEY`, its passphrase `MAVEN_SIGNING_KEY_PASSWORD`.
4. Store them in the environment: `gh secret set MAVEN_CENTRAL_USERNAME --env maven`, the other three likewise (`gh secret set MAVEN_SIGNING_KEY --env maven < key.asc`).

What is published is `io.github.arcships:rutis` (with `rutis-jvm` and `rutis-android`), `io.github.arcships:rutis-ksp` and `io.github.arcships:rutis-testing`, released automatically; the job waits until the deployment is published (an hour at most), and a module the Central Portal has published at this version is skipped (asked through the Portal's publisher API, not repo1.maven.org, which lags behind). The Portal says a version is published only once publishing finished (10 to 30 minutes after the upload), so before uploading the job records in the Actions cache that this version was uploaded: a rerun that finds an earlier upload and a module not published yet waits for it (45 minutes at most) instead of uploading it again, and uploads again only if it is still not published then (a failed deployment, or an upload that never happened). So a rerun is safe. Maven Central can take tens of minutes to show a version in its search; the checks after publishing do not wait for it.

Increment `PLUGIN_API` (present in both the SDK and runtime) only when the interface visible to plugins becomes incompatible. Python plugins depend on `rutis` with a lower bound only and share the host environment's one `rutis` with the runtime: a release that removes or moves a name plugins import says so in its migration guide, and plugins upgrade together with the host. When `PLUGIN_API` increases, the runtimes keep loading the older plugin API (as 0.13 kept loading API 2); where they cannot, the refusal says which SDK version to rebuild the plugin with. Increment the session protocol version (`rutisProtocol` and `rutis_bridge::session::PROTOCOL`) when the wire format becomes incompatible.

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

## 0.18.0

The theme of 0.18.0 is "errors you can understand" (#694): #677, #678, #679, #669, the second half of #676, #687, #688, #690, #702, #726, and the dependency security update #755. All packages are at 0.18.0; see [the release notes](releases/0.18.0.en.md) and [migrating to 0.18](migration-0.18.en.md).

A minor version change of 0.x allows breaking changes, so `cargo-semver-checks` accepts the Rust API changes when it compares with 0.17.2 (`session::Error::Binding`, the `stuck` field of `DisposeWaitError::TimedOut` and so on; see the migration guide). `format` of `rutis.json` stays 1 and the format does not change: the persisted sample is copied from 0.17.2 and its expectations are unchanged; dsh's user layer has not changed, so `cordis.patch.yml` is copied as is. Checking configuration against the Schema (#669) only changes whether a configuration that does not pass can run; the samples' configuration passes. The protocol version is unchanged; it only adds the capability `objects` (objects with methods, used when both ends declare it), and error nodes gain two optional fields, `site` and `rethrown`. The release gate's cross-version tests (`RUTIS_E2E_ONLY_OLDER=1`) meet 0.17.2: a 0.17.2 runtime cannot receive objects, and such a call fails with `bad-value`; under a 0.17.2 host, objects passed between 0.18 runtimes fail with 0.17.2's text. Either way only that call fails.

The run on Dim before release that section 10 of the [configuration Schema design](design-config-schema-2026-10-27.en.md) describes is skipped for this release, by the maintainer's decision.

## 0.17.2

0.17.2 is a patch release: it fixes the problems people using rutis run into that the developer-experience evaluation (#663) found: #665, #666, #667, #668, #672, #675, the first half of #676, #682 (its fourth item moved to 0.18) and #701 (#703–#707 and the fixes after acceptance). All packages are at 0.17.2; see [the release notes](releases/0.17.2.en.md). There is no migration guide: no API is removed or renamed.

A patch release must stay compatible with 0.17.1: `cargo-semver-checks` compares with 0.17.1 and accepts no breaking Rust API change (this release only adds API). `format` of `rutis.json` stays 1 and no configuration changes: the persisted sample is copied from 0.17.1 and its expectations are unchanged; dsh's user layer has not changed, so `cordis.patch.yml` is copied as is. The protocol is unchanged and there is no new capability; the release gate's cross-version tests meet 0.17.1, and a new case, `plugins_of_a_previous_sdk_under_the_current_runtimes`, checks that a plugin built with the previous SDK runs under the current runtimes. From this release, Python plugins declare only a lower bound on `rutis` (#672): while the plugin API version stays the same, Python's `rutis` does not remove or rename names plugins import; the `rutis==<version>` that the guides install for the Python runtime is updated at each release as usual.

## 0.17.1

0.17.1 is a patch release: plugin definitions gain Python, Go, Swift and Kotlin (P2 and P3 of #642: #654, #653), and it fixes #619, #620 (#652) and #621 (#651); the occasional failure of the events conformance case `rutis/row` (#659, seen in a release gate) was the test not waiting long enough, fixed in the test runner by #661. All packages are at 0.17.1; see [the release notes](releases/0.17.1.en.md). There is no migration guide: no API is removed or renamed.

A patch release must stay compatible with 0.17.0: `cargo-semver-checks` compares with 0.17.0 and accepts no breaking Rust API change (this release's Rust changes are all internal). `format` of `rutis.json` stays 1 and no configuration changes: the persisted sample is copied from 0.17.0 and its expectations are unchanged; dsh's user layer has not changed, so `cordis.patch.yml` is copied as is. The protocol is unchanged; the release gate's cross-version tests (`RUTIS_E2E_ONLY_OLDER=1`) meet 0.17.0. A 0.17.0 host already declares `plugins`, so 0.17.1's Python, Go, Swift and Kotlin runtimes can hand over plugin definitions under a 0.17.0 host too; the cross-version tests have no plugin-definition scenario and only decide by `events-v2`, so they need no change. The list of samples the `persisted` tests require gains 0.16.0, 0.17.0 and 0.17.1 (the two previous releases missed it).

## 0.17.0

0.17.0 converges the public Rust API (#617, #643), removes the `build.rs` service list (#647) and adds plugin definitions (P1 of #642: Rust, Node, Bun). All packages are at 0.17.0; see [the release notes](releases/0.17.0.en.md) and [migrating to 0.17](migration-0.17.en.md).

The migration tool `rutis-migrate` is published to crates.io for the first time, last in the train (it depends on no other rutis crate): `CARGO_TOKEN` must be able to publish a new crate. A minor version change of 0.x allows breaking changes, so `cargo-semver-checks` accepts the Rust API changes when it compares with 0.16.0.

`format` of `rutis.json` stays 1 and no configuration changes: the persisted sample is copied from 0.16.0 and its expectations are unchanged; dsh's user layer has not changed, so `cordis.patch.yml` is copied as is. The protocol only adds the capability `plugins`, used when both ends declare it; the release gate's cross-version tests (`RUTIS_E2E_ONLY_OLDER=1`) meet 0.16.0, which does not declare `plugins`, and the session works as before, without plugin definitions.

## 0.16.0

The flat event bus of 0.15.1 stays; the source-compatibility shims that release carried are removed (`Dispatch::scope`, the SDK scope helpers and the testing tools' emitter scope). The release also carries the fixes ready since 0.15.1: cfg-gated service methods, generic alias arguments in exports, dev reload of mixed-language rows, and `Ctx::is_shutting_down()`. All packages are at 0.16.0; see [the release notes](releases/0.16.0.en.md) and [migrating to 0.16](migration-0.16.en.md).

`format` of `rutis.json` stays 1 and no configuration changes: the persisted sample is copied from 0.15.1 and its expectations are unchanged. The event wire format is unchanged; the release gate's cross-version tests (`RUTIS_E2E_ONLY_OLDER=1`) meet 0.15.1. An end still on 0.15.1 keeps its scope filtering, so upgrade all ends for flat delivery.

## 0.15.1

Events are one flat bus again (#604, #605): delivery selects listeners by name only; a scope is a descriptor for filters. This is a behaviour change from 0.13–0.15.0. `Dispatch::scope`, the SDK scope helpers and the testing tools' emitter scope stay source-compatible but are ignored. All packages are at 0.15.1; see [the release notes](releases/0.15.1.en.md) and [migrating to 0.15.1](migration-0.15.1.en.md).

`format` of `rutis.json` stays 1 and no configuration changes: the persisted sample is copied from 0.15.0 and its expectations are unchanged. dsh's user layer has not changed, so `cordis.patch.yml` is copied as is. The event wire format is unchanged; the release gate's cross-version tests (`RUTIS_E2E_ONLY_OLDER=1`) meet 0.15.0 and the sessions work, but an end still on 0.15.0 keeps its scope filtering, so all ends should be upgraded for flat delivery.

## 0.15.0

`rutis-macros` (`#[rutis::service]`, #510) is published to crates.io for the first time, first in the train (`rutis` depends on it): `CARGO_TOKEN` must be able to publish new crates. The Kotlin SDK (`io.github.arcships:rutis`, `io.github.arcships:rutis-ksp`) is published to Maven Central for the first time; when the environment `maven` lacks a secret, that step is skipped and the run summary says so, and everything else is published. The Swift package has no registry: SwiftPM takes the `v0.15.0` tag (the first release with a root `Package.swift`). 0.14 was not released on its own; its work is in this release.

`format` of `rutis.json` stays 1. The persisted sample is copied from 0.13.0 and uses what was added since: `restart` of `runtimes.go` (`"on-failure"`), `runtimes.swift` (`binaries`, `idle`, `restart: "exit"`, the row `swift-MacKit:clipboard`), and a remote runtime's `language` as `"swift"`, `"kotlin"` (iPhone and Android apps that connect in, each with an instanced group) and `"kt"` (an app the host dials, with a node row). The persisted test's expectations still record `restart` only when it is not the default, and `swift` records the same fields as `go`, so the expectations of older samples are unchanged. dsh's user layer has not changed since 0.13.0, so `cordis.patch.yml` is copied as is. The gate's cross-version tests (`RUTIS_E2E_ONLY_OLDER=1`) meet 0.13.0; both ends declare `events-v2` and the meetings work (`bridge/cross-version-meetings`), while releases before 0.13 are still refused (`events/old-peer-refused`).

## 0.13.0

The first release with unified events (events-v2). `format` of `rutis.json` stays 1; the persisted sample is copied from 0.12.1, its node row `office` written the 0.13 way: `export` gains the event prefix `order/*`, and `import: ["stock/changed"]` is added (events and services share `export` / `import`, an item may be a prefix ending in `*`); a node row's config reaches the loader as written, so these two are what its expectations gain. dsh's user layer has not changed since 0.12.1, so `cordis.patch.yml` is copied as is. Unified events (#464) removed `events: { out, in }` from the config of node rows (`rutis-bridge/peer`): no sample of 0.12.1 or earlier writes it, so their expectations are unchanged; an old file writing it is refused by `rutis-host check` and at start, the message naming the rewrite (`the peer config field events was removed in rutis 0.13: …`, see [migrating to 0.13](migration-0.13.en.md#node-links)). This is the first time a removed way of writing puts a sample in `rejected/`: `rejected/peer-events/`, whose `rutis.json` has a node row with `events`, and the `rutis.error` beside it is that message. It is also the first release whose cross-version meetings only check refusals: 0.13 and every earlier release refuse each other at the handshake (`events/old-peer-refused`); the gate (`RUTIS_E2E_ONLY_OLDER=1`) meets 0.12.1, every meeting a refusal. From 0.13.1 the previous release declares `events-v2` too, and the meetings are expected to work again (`bridge/cross-version-meetings`).

## 0.12.0

The persisted `rutis.json` sample gains `restart`, added after 0.11: `"on-failure"` on `runtimes.node`, `"exit"` on `runtimes.py` and `"no"` (the default) on `runtimes.bun`; `format` stays 1. The persisted test's expectations record `restart` when it is not the default, so `"no"` does not appear in them and the older samples' expectations are unchanged. dsh's user layer has not changed since 0.11.0, so `cordis.patch.yml` is copied as is. This is also the first release whose gate runs the cross-version meetings (#426, CI's `e2e` job): the gate sets `RUTIS_E2E_ONLY_OLDER=1`, so they meet only earlier releases, here 0.11.0.

## 0.11.0

Released early so that leaf plugin events (#343) are available sooner. The persisted `rutis.json` sample gains the remote runtime fields added after 0.10: `listen` and `instances` on `runtimes.remote[]` (a runtime that connects in, with a top-level `instanced` group) and `"language": "bun"`; `format` stays 1. From this release the persisted test's expectations also record a remote runtime's `listen` and `instances` (when given), and it checks the runtime names and the configuration of runtimes that connect in; the older samples' expectations are unchanged.

## 0.10.0

The first release that saves its persisted samples in the release PR (the samples of 0.8.0–0.9.1 were added afterwards in #284), and the first in which `rutis.json` has a top-level `format`: an older file without `format` reads as 1, so the 0.10.0 expectations equal 0.9.1's and only the sample gains `"format": 1`. From this release the examples of [Write a Python plugin](guide/python-plugin.en.md) are followed by the check after publishing (#259) instead of by hand.

## 0.9.0

The Bun runtime `@arcships/rutis-bun` and the Go module `go/rutis` are released for the first time. `@arcships/rutis-bun` is a new name on npm: `NPM_TOKEN` must be able to publish new packages under `@arcships`. The Go module is released by the tag `go/rutis/v0.9.0`, which release.yml pushes and the Go module proxy serves from. Until then, projects made by `rutis-host new --lang bun|go` cannot install their dependencies. It is also the first release through the release gate and the check after publishing (#254): the linux-arm64 and darwin-x64 install cells have not run in CI before the tag.

## 0.8.0

From this release the core and the dylib toolchain are in the train: push only `v0.8.0`; the `rutis-v*` tags and publish-rutis.yml are retired. `rutis-sdk`, `rutis-dylib`, `rutis-dylib-meta` and `rutis-dylib-launcher` are published to crates.io for the first time.

## 0.7.0

0.7.0 is the train's first release and builds on core 0.6.1: push only `v0.7.0`, no `rutis-v*`. Apart from `rutis-loader` (0.1.0 before), every package is a new name on its registry: configure pending publishers for `rutis` and `rutis-host` on PyPI first. After the release succeeds, withdraw the old `rutis-interop` from crates.io and npm.

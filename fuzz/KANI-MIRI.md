# Kani and Miri harnesses (#631)

Both are their own workspaces, like `fuzz/`: plain `cargo build` and
`cargo test` in the repository never build them.

## Miri

Miri is installed for the `nightly` toolchain here (0.1.0). The harness
runs the session protocol's pure frame and value path on embedded
fixtures, and the two other in-memory fuzz targets, the lines' framing and
an error graph from the far end; it reads no files, clocks or network, so
Miri's default isolation is enough.

```
RUSTFLAGS="--cfg fuzzing" \
    cargo +nightly miri run --manifest-path fuzz/miri/Cargo.toml
```

`--cfg fuzzing` exposes `rutis_bridge::fuzzing::{session_frame,
local_lines, error_graph}`, the entry points of three cargo-fuzz targets.

The property tests in `crates/rutis-bridge/src/session/protocol.rs` also
run under Miri, with a small case count (proptest reads the clock, so
isolation is off):

```
MIRIFLAGS=-Zmiri-disable-isolation PROPTEST_CASES=2 \
    cargo +nightly miri test -p rutis-bridge --features testing --lib \
    session::protocol::tests::frames_write_and_read_back_unchanged -- --test-threads=1
MIRIFLAGS=-Zmiri-disable-isolation PROPTEST_CASES=2 \
    cargo +nightly miri test -p rutis-bridge --features testing --lib \
    session::protocol::tests::wire_values_write_and_read_back_unchanged -- --test-threads=1
```

## Kani

Kani is installed here: `cargo install --locked kani-verifier` (crates.io,
21 s), then the release bundle and its toolchain.

```
cargo kani --version     # Kani Rust Verifier 0.68.0 (cargo plugin), CBMC 6.11.0
cd fuzz/kani && cargo kani
```

The first-time setup (`cargo kani setup`) downloads a 140,315,343-byte
bundle from the Kani GitHub release. From here that download ran at
30–150 kB/s and stalled repeatedly (a plain `curl` stopped for good after
20 s), so it was fetched with `curl -C -` in a loop — 140 MB in 1919 s,
~73 kB/s — and installed from the local file:

```
# static.rust-lang.org timed out here; rsproxy.cn served it
RUSTUP_DIST_SERVER=https://rsproxy.cn rustup toolchain install nightly-2026-08-21 --profile minimal
cargo kani setup --use-local-bundle /tmp/kani-dl.tar.gz \
    --use-local-toolchain ~/.rustup/toolchains/nightly-2026-08-21-x86_64-unknown-linux-gnu
```

The proofs cover the date and time-of-day constructors, the
days-from-epoch round trip and two instant round trips in
`crates/rutis-bridge/src/session/values.rs`.

**None of them has been verified.** Every path of those constructors formats
into an error string (`format!("invalid time {hour:02}:…")`), and Kani
0.68.0 does not finish on that shape here; `fuzz/kani/limits` holds that shape
and the second one, minimized to no rutis code, next to the controls that do
finish. So the six harnesses run without a verdict; do not report these proofs
as passing until `cargo kani` reports `VERIFICATION:- SUCCESSFUL` for each.

The second shape is `str::from_utf8` over symbolic bytes: it does not finish
either, and `rutis-dylib-meta::parse_boot` runs it on the boot blob's four
fields.

`rutis-sdk` is not Kani-buildable at all. `cargo kani` puts its own
`--sysroot`, `-L` and `--extern force:kani` into `CARGO_ENCODED_RUSTFLAGS` for
the whole graph, and `crates/rutis-sdk/build.rs` panics on a flag it cannot
classify, so a harness crate that depends on the SDK fails in that build
script before Kani sees any code. `boot_meta`, `append`, `is_sha256` and
`rutis_sdk_boot_id` — the four #631 lists besides `parse_boot` — are therefore
unprovable until that const code lives in a crate with nothing to build
(`rutis-dylib-meta`, or a new ABI crate the SDK re-exports): a production
change, not a harness one. `rutis-dylib-meta::parse_boot` itself is private;
reaching it from a harness needs a `#[cfg(kani)]` hook inside
`crates/rutis-dylib-meta/src/lib.rs`, another production file.

The numbers behind all of this are in the last section.

## Results (2026-11-11)

Miri 0.1.0, nightly 1.98.1 host (rustc 1.102.0-nightly), 32 cores; Kani
0.68.0, CBMC 6.11.0, its own toolchain nightly-2026-08-21.

| Command | Result |
| --- | --- |
| `cargo +nightly miri run --manifest-path fuzz/miri/Cargo.toml` (with `RUSTFLAGS=--cfg fuzzing`), 30 inputs: 13 frames, one past the depth limit, 9 line-framing seeds, 7 error graphs | `miri: ok`, no UB, no panic, 5 s |
| `cargo +nightly miri test -p rutis-bridge --features testing --lib session::protocol::tests::frames_write_and_read_back_unchanged -- --test-threads=1` (`PROPTEST_CASES=2`, `MIRIFLAGS=-Zmiri-disable-isolation`) | 1 passed, 122 s |
| same, `wire_values_write_and_read_back_unchanged` | 1 passed, 31 s |
| `cargo +nightly miri test -p rutis-bridge --features testing --lib session::rpc::waits -- --test-threads=1` (`MIRIFLAGS=-Zmiri-disable-isolation`) | 15 passed, 1 ignored (`measure_the_check`, a measurement), 846 s |
| `cargo +nightly fuzz run session_frame <copy of corpus/session_frame> -- -max_total_time=60` | 3,256,860 runs, no crash |
| `cargo +nightly fuzz run local_lines <copy of corpus/local_lines> -- -max_total_time=60` | 1,973,957 runs, no crash |
| `cargo +nightly fuzz run error_graph <copy of corpus/error_graph> -- -max_total_time=60` | 4,969,318 runs, no crash |
| `cargo +nightly fuzz run websocket_upgrade <copy of corpus/websocket_upgrade> -- -max_total_time=60` | 3,576,965 runs, no crash |
| `cargo kani` in a scratch crate: `assert_eq!` on `u8` (Kani smoke test) | `VERIFICATION:- SUCCESSFUL`, 0 s |
| `cd fuzz/kani && cargo kani -j 6` (the 6 harnesses: `date_new_keeps_valid_fields`, `date_new_rejects_month_and_day_zero`, `days_from_epoch_roundtrip`, `time_of_day_keeps_valid_fields`, `instant_reads_back_through_utc`, `instant_nanoseconds_roundtrip`) | no verdict: stopped after 5400 s (90 min) with all six still checking; `date_new_keeps_valid_fields` alone also ran over 10 min without a verdict before that |

The fuzz runs used a copy of each corpus directory, so the fuzzer's new
inputs did not change `fuzz/corpus/`.

The Kani harnesses are committed as the properties to prove once a Kani
release finishes them; the run above is the current state, not a pass.

## Results (2026-11-16, dylib)

| Command | Result |
| --- | --- |
| `cargo +nightly fuzz run dylib_meta <copy of corpus/dylib_meta> -- -max_total_time=60` | 1,715,852 runs, no crash |
| `cargo +nightly fuzz run binary_manifest <copy of corpus/binary_manifest> -- -max_total_time=60` | 4,114,060 runs, no crash |

The two targets read what the dylib loader checks before `dlopen`
(`crates/rutis-dylib-meta`: ELF, Mach-O and PE identity and dependencies)
and the manifest a runtime binary prints (`ManifestWire`, checked before
any field is used). This slice adds no Kani harness for `boot_meta`,
`append`, `is_sha256` or `parse_boot`: the six harnesses above still have
no verdict, so no proof is reported as passing. The section below says why.

## Results (2026-11-16, dsh profile and Go frames)

| Command | Result |
| --- | --- |
| `cargo +nightly fuzz run dsh_profile_yaml <copy of corpus/dsh_profile_yaml> -- -max_total_time=60 -timeout=10` | 951,596 runs, no crash, after the fixes below |
| `cargo +nightly fuzz run dsh_profile_expr <copy of corpus/dsh_profile_expr> -- -max_total_time=60 -timeout=10` | 2,299,974 runs, no crash |
| `go test ./internal/runtime -run '^$' -fuzz '^FuzzFrameParsing$' -fuzztime 60s` (in `go/rutis`) | 2,468,695 executions, PASS |
| `go test ./internal/runtime` (in `go/rutis`) | the seeds pass |

The first YAML run found a render mismatch (#1234) and three panics in
`Document::parse`'s line slicing (#1235, #1237, #1238). The panics are
fixed, with the inputs as a regression test in
`crates/rutis-dsh/tests/profile_yaml.rs`; #1234 is a remaining mismatch
(`"-\n n"` is read as one item and rendered as two), so the target parses
and renders but no longer compares the rendered values. The expression
target evaluates with an empty environment, `cwd` fixed and no services,
so it reads no live process state.

The fuzz runs used a copy of each corpus directory, so the fuzzer's new
inputs did not change `fuzz/corpus/`.

## Results (2026-11-17, what Kani 0.68.0 does not finish)

Kani Rust Verifier 0.68.0 (cargo plugin), CBMC 6.11.0, its own toolchain
nightly-2026-08-21 — already the newest release
(`api.github.com/repos/model-checking/kani/releases/latest` names
`kani-0.68.0`), so there is no upgrade to try. The install works: a scratch
crate's `let x: u8 = kani::any(); assert!(x == x);` verifies in 0.05 s.

`fuzz/kani/limits` holds the two shapes that do not finish, minimized to no
rutis code, each its own `--harness`:

| Command | Result |
| --- | --- |
| `cd fuzz/kani/limits && cargo kani --harness format_literal` (`format!("bad")`) | `VERIFICATION:- SUCCESSFUL`, 2 s |
| `--harness display_to_string` (`let x: u8 = kani::any(); x.to_string()`) | `VERIFICATION:- SUCCESSFUL`, 1 s |
| `--harness plain_byte_loop` (`while` over 8 symbolic bytes) | `VERIFICATION:- SUCCESSFUL`, 0 s |
| `--harness format_with_argument` (`format!("bad {x}")`, `x: u8`) | no verdict, `timeout 120` killed it; 240 s in an earlier run |
| `--harness utf8_one_byte` (`std::str::from_utf8` over 1 symbolic byte) | no verdict, `timeout 120` killed it; 2, 4 and 8 symbolic bytes likewise |
| `cd fuzz/kani && cargo kani --harness probe_assume_skips_error_path` (`Date::new` with `year` and `month` symbolic but `kani::assume`d valid, day fixed at 15, so the error path is unreachable) | no verdict, `timeout 200` killed it — the error branch is explored anyway, its condition is symbolic |
| `cd fuzz/kani && cargo kani -j 6 --output-format terse` (the six harnesses) | no verdict after 420 s, all six at `Checking harness` |
| a scratch crate whose only harness is `assert_eq!(rutis_dylib_meta::BOOT_SIZE, 512)` | `VERIFICATION:- SUCCESSFUL`, 0.01 s: the crate and its `object` dependency build under Kani |
| a scratch crate depending on `rutis-sdk`, harness `!rutis_sdk::is_sha256("abc")` | build fails: `crates/rutis-sdk/build.rs:163:13: panicked: unclassified RUSTFLAGS argument: --sysroot` |

So the two shapes are `format!` with an argument and `str::from_utf8` over
symbolic bytes — not formatting, not allocation, not a loop over symbolic
data, each of which the controls above show finishing in a second or two.
Formatting the number alone (`x.to_string()`) is fine; the `format!`
expansion is not.

What that blocks, and why no harness shape gets around it:

- **The six bridge harnesses.** Every path of `Date::new`, `TimeOfDay::new`,
  `Date::from_days_since_unix_epoch` and `Instant::from_unix` formats its
  error message, and `kani::assume` cannot keep the verifier out of that
  branch (last row of the table): the branch condition is symbolic, so the
  message is symbolically executed before the assumption prunes the path.
  A fix is production work: a `fn valid(..) -> bool` the constructors call
  before building the message, and which a harness can call on its own.
  Stubbing `core::fmt` for the harness (`-Z stubbing`) was not tried.
- **`rutis-dylib-meta::parse_boot`.** The crate itself builds under Kani
  (table above), but `parse_boot` is private: reaching it needs a `#[cfg(kani)]`
  hook in `crates/rutis-dylib-meta/src/lib.rs`, a production file. Even with
  the hook, its four fields go through `str::from_utf8` on symbolic bytes —
  the second shape — so the proof would need the boot blob's content held to
  a concrete alphabet, or a validation path that does not call `from_utf8`.
- **`rutis-sdk`'s `boot_meta`, `append`, `is_sha256` and `rutis_sdk_boot_id`.**
  The crate cannot be built by `cargo kani` at all: `cargo kani` puts
  `--sysroot=<kani>`, `-L <kani>/lib` and `--extern force:kani` into
  `CARGO_ENCODED_RUSTFLAGS` for the whole graph, and
  `crates/rutis-sdk/build.rs` — which canonicalizes the flags so the SDK is
  reproducible — panics on a flag it does not classify. That script is right
  to be strict; what is wrong is that the const ABI lives in the crate that
  has to be built. Moving `BOOT_MAGIC`, `BOOT_SIZE`, `is_sha256`, `append`
  and `boot_meta` into a crate with nothing to build (`rutis-dylib-meta`, or
  a new ABI crate) and re-exporting them from `rutis-sdk` would make all four
  provable, since the logic itself is a loop over at most 64 bytes and would
  need no `format!` or `from_utf8` at all. That is a production change and
  was not made here.

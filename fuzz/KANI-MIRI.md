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

**None of them has been verified.** Each harness calls a constructor whose
error path formats a number (`format!("invalid time {hour:02}:…")`), and
`cargo kani` under this release does not finish on that shape: a toy
harness in a scratch crate whose whole subject is `format!("bad {x}")`
with `x: u8` — no rutis code — does not finish in 5 minutes, while the
same with no argument (`format!("bad")`, 2 s), `String::from` (`String`/
`Vec` only, 0.5 s) and `assert_eq!` on `u8` (0 s) all verify at once. So
the six harnesses run without a verdict; do not report these proofs as
passing until `cargo kani` reports `VERIFICATION:- SUCCESSFUL` for each.

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
no verdict, so no proof is reported as passing.

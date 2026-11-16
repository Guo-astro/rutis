//! The two shapes `cargo kani` (0.68.0, CBMC 6.11.0) does not finish on this
//! machine, minimized to no rutis code, next to the controls that do. Each
//! harness is its own `--harness`; run one at a time, they have no verdict:
//!
//!     cargo kani --harness format_with_argument   # does not finish
//!     cargo kani --harness utf8_one_byte          # does not finish
//!     cargo kani --harness format_literal         # VERIFICATION:- SUCCESSFUL
//!
//! These are not properties of this crate: they are why the proofs in
//! `../src/lib.rs` and the ones #631 asks for (the SDK's `boot_meta`,
//! `append`, `is_sha256`, `rutis_sdk_boot_id`, `rutis-dylib-meta`'s
//! `parse_boot`) have no verdict — every path of those functions formats a
//! number into an error string or validates a symbolic UTF-8 field.
//!
#![cfg(kani)]

/// `format!` on a literal alone verifies at once, so the cost is not
/// allocation or formatting in general.
#[kani::proof]
fn format_literal() {
    let text = format!("bad");
    assert_eq!(text.len(), 3);
}

/// `format!` with one symbolic argument does not finish: the error paths of
/// `Date::new`, `TimeOfDay::new`, `Instant::from_unix` and the rest of
/// `rutis-bridge`'s date and time-of-day constructors all take this shape.
#[kani::proof]
fn format_with_argument() {
    let value: u8 = kani::any();
    let text = format!("bad {value}");
    assert!(text.len() >= 3);
}

/// Display of the same symbolic number on its own verifies at once, so a
/// numeric formatting routine is not the cost either.
#[kani::proof]
fn display_to_string() {
    let value: u8 = kani::any();
    let text = value.to_string();
    assert!(!text.is_empty());
}

/// A plain loop over eight symbolic bytes verifies at once.
#[kani::proof]
fn plain_byte_loop() {
    let bytes: [u8; 8] = kani::any();
    let mut i = 0;
    let mut ascii = true;
    while i < bytes.len() {
        if bytes[i] > 0x7f {
            ascii = false;
        }
        i += 1;
    }
    assert!(ascii || !ascii);
}

/// `str::from_utf8` over a single symbolic byte does not finish; over two,
/// four or eight it does not either. `rutis-dylib-meta::parse_boot` runs it
/// on the four fields of the boot blob.
#[kani::proof]
fn utf8_one_byte() {
    let bytes: [u8; 1] = kani::any();
    assert!(std::str::from_utf8(&bytes).is_ok() || std::str::from_utf8(&bytes).is_err());
}

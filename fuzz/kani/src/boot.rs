//! Proofs of the plugin boot blob's const ABI (#1271), the code
//! `export_plugin!` embeds and the host's C ABI query reads back.
//!
//! `boot_meta` and `is_sha256` take `&str`, and Kani has no symbolic string:
//! the proofs call the byte-level functions those two delegate to, with
//! concrete lengths and symbolic content. The `&str` wrappers add
//! `.as_bytes()` and nothing else, so what is proved here is the layout and
//! the predicate; `crates/rutis-abi`'s tests check the wrappers against the
//! same layout written out by hand.
#![cfg(kani)]

use rutis_abi::{append, boot_meta_bytes, is_sha256_bytes, write_boot_id, BOOT_SIZE};

/// The SDK's own id shape: 64 lowercase hex characters.
const ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const MAGIC: [u8; 21] = *b"RUTIS_PLUGIN_BOOT_V1\0";

/// One boot field as the layout writes it: a `u16` little-endian length, then
/// the bytes. The field is an array, so the length is a constant and no loop
/// bound is symbolic.
macro_rules! encode {
    ($out:expr, $pos:expr, $field:expr) => {{
        let field = $field;
        $out[$pos] = field.len() as u8;
        $out[$pos + 1] = (field.len() >> 8) as u8;
        let mut i = 0;
        while i < field.len() {
            $out[$pos + 2 + i] = field[i];
            i += 1;
        }
        $pos + 2 + field.len()
    }};
}

/// The whole blob: the magic, the four fields in order, and zeros after them.
/// Byte loops with array lengths, not slice `==` on slices whose length is
/// read at run time: that comparison's `memcmp` is unwound for thousands of
/// iterations (`fuzz/kani/limits`).
#[kani::proof]
fn boot_meta_lays_out_four_fields() {
    let sdk_id: [u8; 64] = kani::any();
    let artifact: [u8; 64] = kani::any();
    let id: [u8; 16] = kani::any();
    let version: [u8; 10] = kani::any();
    let boot = boot_meta_bytes(&sdk_id, &artifact, &id, &version);
    let mut expected = [0_u8; BOOT_SIZE];
    let mut i = 0;
    while i < MAGIC.len() {
        expected[i] = MAGIC[i];
        i += 1;
    }
    let mut pos = MAGIC.len();
    pos = encode!(expected, pos, sdk_id);
    pos = encode!(expected, pos, artifact);
    pos = encode!(expected, pos, id);
    pos = encode!(expected, pos, version);
    let mut i = 0;
    while i < BOOT_SIZE {
        assert_eq!(boot[i], expected[i]);
        i += 1;
    }
    let _ = pos;
}

/// One field at a concrete position and length: the length goes in front, the
/// bytes after it, and nothing before or after the field changes.
#[kani::proof]
fn append_encodes_one_field() {
    let field: [u8; 32] = kani::any();
    let mut out = [0xff_u8; BOOT_SIZE];
    let end = append(&mut out, 18, &field);
    assert_eq!(end, 52);
    let mut expected = [0xff_u8; BOOT_SIZE];
    let _ = encode!(expected, 18, field);
    let mut i = 0;
    while i < BOOT_SIZE {
        assert_eq!(out[i], expected[i]);
        i += 1;
    }
}

/// 64 bytes are a SHA-256 in text exactly when every one of them is a digit
/// or a lowercase letter `a`–`f`.
#[kani::proof]
fn is_sha256_only_lowercase_hex() {
    let bytes: [u8; 64] = kani::any();
    let accepted = is_sha256_bytes(&bytes);
    let mut expected = true;
    let mut i = 0;
    while i < 64 {
        let byte = bytes[i];
        if !((byte >= b'0' && byte <= b'9') || (byte >= b'a' && byte <= b'f')) {
            expected = false;
        }
        i += 1;
    }
    assert_eq!(accepted, expected);
}

/// The C ABI body writes the id when the capacity covers it and leaves the
/// buffer alone when it does not; either way it returns the id's length.
#[kani::proof]
fn boot_id_writes_all_or_nothing() {
    let mut buf = [0xff_u8; 64];
    let cap: usize = kani::any();
    kani::assume(cap <= 64);
    let len = unsafe { write_boot_id(ID, buf.as_mut_ptr(), cap) };
    assert_eq!(len, 64);
    if cap == 64 {
        let mut i = 0;
        while i < 64 {
            assert_eq!(buf[i], ID.as_bytes()[i]);
            i += 1;
        }
    } else {
        let mut i = 0;
        while i < 64 {
            assert_eq!(buf[i], 0xff);
            i += 1;
        }
    }
}

/// A null buffer is not written to, whatever the capacity.
#[kani::proof]
fn boot_id_accepts_a_null_buffer() {
    let cap: usize = kani::any();
    assert_eq!(unsafe { write_boot_id(ID, core::ptr::null_mut(), cap) }, 64);
}

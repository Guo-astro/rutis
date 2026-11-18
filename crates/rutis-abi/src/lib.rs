//! The const ABI of the rutis plugin boot blob: the section `export_plugin!`
//! writes into a plugin dylib, which `rutis-dylib-meta` reads back before
//! `dlopen`.
//!
//! Nothing here has a dependency, a build script or a use for the standard
//! library, so `cargo kani` builds this crate and the proofs in `fuzz/kani`
//! call the code that ships instead of a copy of it (#1271). `rutis-sdk`
//! re-exports what plugin and host code used to reach there, so neither sees
//! a change.
#![no_std]

/// The first bytes of the boot section, at the end of which the fields
/// start.
pub const BOOT_MAGIC: &[u8] = b"RUTIS_PLUGIN_BOOT_V1\0";

/// The section's size: the fields are zero-padded to it.
pub const BOOT_SIZE: usize = 512;

/// Whether `value` is the text form of a SHA-256: 64 lowercase hexadecimal
/// characters.
pub const fn is_sha256(value: &str) -> bool {
    is_sha256_bytes(value.as_bytes())
}

/// [`is_sha256`] over the bytes of a text; the predicate Kani proves.
pub const fn is_sha256_bytes(value: &[u8]) -> bool {
    if value.len() != 64 {
        return false;
    }
    let mut i = 0;
    while i < value.len() {
        if !((value[i] >= b'0' && value[i] <= b'9') || (value[i] >= b'a' && value[i] <= b'f')) {
            return false;
        }
        i += 1;
    }
    true
}

/// Writes `bytes` at `pos` as a `u16` little-endian length and then the
/// bytes, and returns the position after them: the encoding of one boot
/// field.
///
/// # Panics
///
/// When a field is longer than `u16::MAX` bytes, or the field and its length
/// do not fit in what is left of `out`.
pub const fn append(out: &mut [u8; BOOT_SIZE], mut pos: usize, bytes: &[u8]) -> usize {
    assert!(bytes.len() <= u16::MAX as usize);
    assert!(pos + 2 + bytes.len() <= BOOT_SIZE);
    let len = bytes.len() as u16;
    out[pos] = (len & 255) as u8;
    out[pos + 1] = (len >> 8) as u8;
    pos += 2;
    let mut i = 0;
    while i < bytes.len() {
        out[pos + i] = bytes[i];
        i += 1;
    }
    pos + bytes.len()
}

/// The boot section `export_plugin!` embeds: [`BOOT_MAGIC`], then `sdk_id`,
/// `artifact`, `id` and `version` as fields, zero-padded to [`BOOT_SIZE`].
pub const fn boot_meta(sdk_id: &str, artifact: &str, id: &str, version: &str) -> [u8; BOOT_SIZE] {
    boot_meta_bytes(
        sdk_id.as_bytes(),
        artifact.as_bytes(),
        id.as_bytes(),
        version.as_bytes(),
    )
}

/// [`boot_meta`] over the bytes of the four fields: the layout Kani proves.
///
/// # Panics
///
/// When a field is longer than `u16::MAX` bytes, or the four fields and their
/// lengths do not fit in [`BOOT_SIZE`].
pub const fn boot_meta_bytes(
    sdk_id: &[u8],
    artifact: &[u8],
    id: &[u8],
    version: &[u8],
) -> [u8; BOOT_SIZE] {
    let mut out = [0; BOOT_SIZE];
    let mut i = 0;
    while i < BOOT_MAGIC.len() {
        out[i] = BOOT_MAGIC[i];
        i += 1;
    }
    let mut pos = BOOT_MAGIC.len();
    pos = append(&mut out, pos, sdk_id);
    pos = append(&mut out, pos, artifact);
    pos = append(&mut out, pos, id);
    let _ = append(&mut out, pos, version);
    out
}

/// Writes `id` into `buf` and returns its length in bytes, all of it or
/// none: the body of `rutis_sdk_boot_id`, which passes the SDK's own id.
///
/// # Safety
///
/// `buf` is null or points to at least `cap` writable bytes.
pub unsafe fn write_boot_id(id: &str, buf: *mut u8, cap: usize) -> usize {
    let bytes = id.as_bytes();
    if cap >= bytes.len() && !buf.is_null() {
        // SAFETY: the caller guarantees `cap` writable bytes at `buf`, and
        // they cover the id.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len()) };
    }
    bytes.len()
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// The layout written down by hand, as `rutis-dylib-meta` reads it.
    fn blob(fields: [&[u8]; 4]) -> Vec<u8> {
        let mut out = BOOT_MAGIC.to_vec();
        for field in fields {
            out.extend_from_slice(&(field.len() as u16).to_le_bytes());
            out.extend_from_slice(field);
        }
        out.resize(BOOT_SIZE, 0);
        out
    }

    #[test]
    fn boot_meta_is_the_wire_layout() {
        let (sdk_id, artifact, id, version) = ("sdk", "artifact", "greeter", "1.2.3");
        assert_eq!(
            boot_meta(sdk_id, artifact, id, version).to_vec(),
            blob([
                sdk_id.as_bytes(),
                artifact.as_bytes(),
                id.as_bytes(),
                version.as_bytes()
            ])
        );
    }

    #[test]
    fn boot_meta_keeps_the_magic_and_zero_pads() {
        let boot = boot_meta("sdk", "artifact", "id", "1");
        assert!(boot.starts_with(BOOT_MAGIC));
        assert_eq!(boot.len(), BOOT_SIZE);
        let end = BOOT_MAGIC.len() + (2 + 3) + (2 + 8) + (2 + 2) + (2 + 1);
        assert!(boot[end..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn append_writes_a_length_then_the_bytes() {
        let mut out = [0xff; BOOT_SIZE];
        let pos = append(&mut out, 4, b"abc");
        assert_eq!(pos, 9);
        assert_eq!(&out[4..9], &[3, 0, b'a', b'b', b'c']);
        assert!(out[9..].iter().all(|byte| *byte == 0xff));
    }

    #[test]
    fn is_sha256_accepts_only_64_lowercase_hex() {
        assert!(is_sha256(&"0123456789abcdef".repeat(4)));
        assert!(!is_sha256(&"0123456789abcdef".repeat(3)));
        assert!(!is_sha256(&"0123456789ABCDEF".repeat(4)));
        assert!(!is_sha256(&std::format!("{}g", "0".repeat(63))));
    }

    #[test]
    fn write_boot_id_writes_all_or_nothing() {
        let id = "a".repeat(64);
        let mut buf = [0xff; 64];
        assert_eq!(
            unsafe { write_boot_id(&id, buf.as_mut_ptr(), buf.len()) },
            64
        );
        assert_eq!(buf.as_slice(), id.as_bytes());
        let mut short = [0xff; 64];
        assert_eq!(unsafe { write_boot_id(&id, short.as_mut_ptr(), 63) }, 64);
        assert!(short.iter().all(|byte| *byte == 0xff));
        assert_eq!(unsafe { write_boot_id(&id, core::ptr::null_mut(), 64) }, 64);
    }
}

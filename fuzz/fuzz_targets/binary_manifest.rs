//! What a runtime binary prints for `--rutis-manifest`, read as a host does
//! before it trusts the binary (#631): arbitrary bytes are an error, never a
//! panic, and a format this host does not read is refused whole
//! (`rutis_loader::runtime::binary::fuzzing::binary_manifest`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    rutis_loader::runtime::binary::fuzzing::binary_manifest(data)
});

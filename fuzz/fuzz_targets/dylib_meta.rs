//! A plugin file read before `dlopen` (#631): what it says about itself and
//! what it links, in every object format the loader supports. Arbitrary
//! bytes give an error, never a panic, and each read is bounded by the
//! input (`rutis_dylib_meta`).
#![no_main]

use rutis_dylib_meta::{
    check_plugin_dependencies, exported_symbols, library_file_name, needed_libraries, read_boot,
    sdk_reference, std_reference, weak_rust_exports, SharedLibraries,
};

/// The SDK and libstd file names a plugin built for each target records;
/// `tests/dylib-fixtures/greeter-v1` is built for all three.
const TARGETS: [(&str, &str, &str); 3] = [
    (
        "x86_64-unknown-linux-gnu",
        "librutis_sdk.so",
        "libstd-0123456789abcdef.so",
    ),
    (
        "aarch64-apple-darwin",
        "@rpath/librutis_sdk.dylib",
        "@rpath/libstd-0123456789abcdef.dylib",
    ),
    (
        "x86_64-pc-windows-msvc",
        "rutis_sdk.dll",
        "std-0123456789abcdef.dll",
    ),
];

/// Plugin files can be tens of megabytes; a parsed prefix is enough to
/// reach every branch, and the bound keeps one input's work small.
const MAX_LEN: usize = 1 << 20;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_LEN {
        return;
    }
    for (target, sdk, std) in TARGETS {
        let shared = SharedLibraries { sdk, std };
        let _ = read_boot(data, target);
        let _ = exported_symbols(data, target);
        let _ = needed_libraries(data, target);
        let _ = check_plugin_dependencies(data, target, &shared);
        let _ = weak_rust_exports(data, target);
        let _ = std_reference(data, target);
        let _ = sdk_reference(target);
        let _ = library_file_name("rutis-sdk", target);
    }
});

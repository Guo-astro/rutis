//! The Swift SDK's binaries for the session-end suites (`swift` in
//! `RUTIS_TEST_RUNTIMES`): a product of the root `Package.swift`, built
//! here once per test binary with the `swift` on PATH. The runtime it
//! serves is `Rutis.serve`'s, as every Swift runtime's is.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The product `product` of the Swift package (the root `Package.swift`),
/// built once per test binary.
pub fn binary(product: &str) -> PathBuf {
    /// The products this test binary built already.
    static BUILT: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    // A test that panicked while it built must not poison this one: it builds
    // the product again instead of failing on the lock.
    let mut built = BUILT
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !built.iter().any(|built| built == product) {
        let status = std::process::Command::new("swift")
            .args(["build", "--product", product])
            .current_dir(repo())
            .status()
            .expect("swift is on PATH");
        assert!(status.success(), "swift build --product {product}");
        built.push(product.to_owned());
    }
    drop(built);
    let output = std::process::Command::new("swift")
        .args(["build", "--show-bin-path"])
        .current_dir(repo())
        .output()
        .expect("swift is on PATH");
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim()).join(product)
}

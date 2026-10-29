//! Swift runtimes: one per binary built with the Swift SDK (`swift/rutis`),
//! several side by side (docs/design-swift-sdk-2026-10-12.md §3).
//!
//! Swift is a [`BinaryLanguage`]: rows `swift:<plugin>`, the SDK's marker `rutis-swift-runtime:1`,
//! runtimes started by [`Launcher::swift`]. Everything else (manifests,
//! directory scans, the lifecycle, restarts, replaced and refused binaries)
//! is [`super::binary`]'s, shared with Go: `BinaryResolver<Swift>`,
//! `BinaryRuntimes<Swift>` and so on.
//!
//! A runtime is named after its file (`swift-<name>`, see
//! [`rutis_bridge::runtime::swift_runtime_name`]). While a runtime runs,
//! rows resolve from the manifest it started with; a binary replaced on
//! disk takes effect when the runtime restarts.

use std::path::Path;

use rutis_bridge::runtime::{swift_runtime_name, Launcher};

use super::binary::BinaryLanguage;

/// The plugin API of Swift binaries this host runs, read from their
/// manifests.
#[doc(hidden)]
pub const SWIFT_PLUGIN_API: u64 = 3;

/// Swift as a [`BinaryLanguage`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Swift;

impl BinaryLanguage for Swift {
    const PREFIX: &'static str = "swift";
    const TITLE: &'static str = "Swift";
    /// In every binary built with the Swift SDK: only files containing it are
    /// run to read their manifest.
    const MARKER: &'static [u8] = b"rutis-swift-runtime:1";
    const PLUGIN_API: u64 = SWIFT_PLUGIN_API;

    fn runtime_name(path: &Path) -> String {
        swift_runtime_name(path)
    }

    fn launch(binary: &Path, project: &Path) -> Launcher {
        Launcher::swift(binary, project)
    }
}

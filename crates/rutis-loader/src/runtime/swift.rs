//! Swift runtimes: one per binary built with the Swift SDK (`swift/rutis`),
//! several side by side (docs/design-swift-sdk-2026-10-12.md §3).
//!
//! Swift is a [`BinaryLanguage`]: rows `swift:<plugin>` and
//! `swift-<binary>:<plugin>`, the SDK's marker `rutis-swift-runtime:1`,
//! runtimes started by [`LocalRuntime::swift`]. Everything else (manifests,
//! directory scans, the lifecycle, restarts, replaced and refused binaries)
//! is [`super::binary`]'s, shared with Go.
//!
//! A runtime is named after its file (`swift-<name>`, see
//! [`rutis_bridge::runtime::swift_runtime_name`]). While a runtime runs,
//! rows resolve from the manifest it started with; a binary replaced on
//! disk takes effect when the runtime restarts.

use std::path::Path;

use rutis_bridge::runtime::{swift_runtime_name, LocalRuntime};

use super::binary::{
    read_binary_manifest, Binaries, BinaryFile, BinaryLanguage, BinaryManifest, BinaryResolver,
    BinaryRuntimeInfo, BinaryRuntimeState, BinaryRuntimes, BinaryRuntimesHandle,
};

/// In every binary built with the Swift SDK: only files containing it are
/// run to read their manifest.
pub const SWIFT_MARKER: &[u8] = b"rutis-swift-runtime:1";

/// The plugin API of Swift binaries this host runs, read from their
/// manifests.
pub const SWIFT_PLUGIN_API: u64 = 3;

/// Swift as a [`BinaryLanguage`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Swift;

impl BinaryLanguage for Swift {
    const PREFIX: &'static str = "swift";
    const TITLE: &'static str = "Swift";
    const MARKER: &'static [u8] = SWIFT_MARKER;
    const PLUGIN_API: u64 = SWIFT_PLUGIN_API;

    fn runtime_name(path: &Path) -> String {
        swift_runtime_name(path)
    }

    fn launch(binary: &Path, project: &Path) -> LocalRuntime {
        LocalRuntime::swift(binary, project)
    }
}

/// Where Swift binaries are: files, and directories whose Swift binaries
/// all count.
pub type SwiftBinaries = Binaries;

/// What a Swift binary's manifest says.
pub type SwiftManifest = BinaryManifest;

/// A Swift binary as [`SwiftResolver::binaries`] reports it.
pub type SwiftBinary = BinaryFile;

/// Resolves `swift:<plugin>` and `<runtime>:<plugin>` rows to Swift binaries.
pub type SwiftResolver = BinaryResolver<Swift>;

/// Starts the Swift runtimes rows use, one per binary.
pub type SwiftRuntimes = BinaryRuntimes<Swift>;

/// Controls the Swift runtimes from outside the plugin.
pub type SwiftRuntimesHandle = BinaryRuntimesHandle<Swift>;

/// Where a Swift runtime is.
pub type SwiftRuntimeState = BinaryRuntimeState;

/// A Swift runtime as [`SwiftRuntimesHandle::runtimes`] reports it.
pub type SwiftRuntimeInfo = BinaryRuntimeInfo;

/// What the Swift binary at `path` serves: it runs `path --rutis-manifest`
/// (within 30 s).
pub fn read_swift_manifest(path: &Path) -> Result<SwiftManifest, String> {
    read_binary_manifest(path)
}

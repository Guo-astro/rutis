//! Go runtimes: one per binary built with the Go SDK (`go/rutis`), several
//! side by side, each started when a row uses it.
//!
//! Go is a [`BinaryLanguage`]: rows `go:<plugin>` and `go-<binary>:<plugin>`,
//! the SDK's marker `rutis-go-runtime:1`, runtimes started by
//! [`LocalRuntime::go`]. Everything else (manifests, directory scans, the
//! on-demand lifecycle, restarts, replaced and refused binaries) is
//! [`super::binary`]'s; the names here are the Go ones it has always had.
//!
//! A runtime is named after its file (`go-<name>`, see
//! [`rutis_bridge::runtime::go_runtime_name`]): that is the name rows,
//! [`GoRuntimesHandle::restart`] and diagnostics use. While a runtime runs,
//! rows resolve from the manifest it started with; a binary replaced on
//! disk takes effect when the runtime restarts.

use std::path::Path;

use rutis_bridge::runtime::{go_runtime_name, LocalRuntime};

use super::binary::{
    read_binary_manifest, Binaries, BinaryFile, BinaryLanguage, BinaryManifest, BinaryResolver,
    BinaryRuntimeInfo, BinaryRuntimeState, BinaryRuntimes, BinaryRuntimesHandle,
};

/// In every binary built with the Go SDK: only files containing it are run
/// to read their manifest.
#[doc(hidden)]
pub const GO_MARKER: &[u8] = b"rutis-go-runtime:1";

/// The plugin API of Go binaries this host runs, read from their manifests.
/// The one definition: `rutis-host` uses it for the same check.
#[doc(hidden)]
pub const PLUGIN_API: u64 = 3;

/// Go as a [`BinaryLanguage`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Go;

impl BinaryLanguage for Go {
    const PREFIX: &'static str = "go";
    const TITLE: &'static str = "Go";
    const MARKER: &'static [u8] = GO_MARKER;
    const PLUGIN_API: u64 = PLUGIN_API;

    fn runtime_name(path: &Path) -> String {
        go_runtime_name(path)
    }

    fn launch(binary: &Path, project: &Path) -> LocalRuntime {
        LocalRuntime::go(binary, project)
    }
}

/// Where Go binaries are: files, and directories whose Go binaries all
/// count.
pub type GoBinaries = Binaries;

/// What a Go binary's manifest says.
pub type GoManifest = BinaryManifest;

/// A Go binary as [`GoResolver::binaries`] reports it.
pub type GoBinary = BinaryFile;

/// Resolves `go:<plugin>` and `<runtime>:<plugin>` rows to Go binaries.
pub type GoResolver = BinaryResolver<Go>;

/// Starts the Go runtimes rows use, one per binary, and stops them once
/// idle.
pub type GoRuntimes = BinaryRuntimes<Go>;

/// Controls the Go runtimes from outside the plugin.
pub type GoRuntimesHandle = BinaryRuntimesHandle<Go>;

/// Where a Go runtime is.
pub type GoRuntimeState = BinaryRuntimeState;

/// A Go runtime as [`GoRuntimesHandle::runtimes`] reports it.
pub type GoRuntimeInfo = BinaryRuntimeInfo;

/// What the Go binary at `path` serves: it runs `path --rutis-manifest`
/// (within 30 s).
#[doc(hidden)]
pub fn read_go_manifest(path: &Path) -> Result<GoManifest, String> {
    read_binary_manifest(path)
}

//! Go runtimes: one per binary built with the Go SDK (`go/rutis`), several
//! side by side, each started when a row uses it.
//!
//! Go is a [`BinaryLanguage`]: rows `go:<plugin>`, the SDK's marker `rutis-go-runtime:1`, runtimes started by
//! the binary itself, in the project, on an inherited socket. Everything else (manifests, directory scans, the
//! on-demand lifecycle, restarts, replaced and refused binaries) is
//! [`super::binary`]'s: `BinaryResolver<Go>`, `BinaryRuntimes<Go>` and so on.
//!
//! A runtime is named after its file (`go-<name>`, see
//! [`go_runtime_name`]): that is the name
//! `BinaryRuntimesHandle::restart` and diagnostics use. While a runtime runs,
//! rows resolve from the manifest it started with; a binary replaced on
//! disk takes effect when the runtime restarts.

use std::path::Path;

use rutis_bridge::runtime::Launcher;

use super::binary::BinaryLanguage;

/// Go as a [`BinaryLanguage`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Go;

impl BinaryLanguage for Go {
    const PREFIX: &'static str = "go";
    const TITLE: &'static str = "Go";
    /// In every binary built with the Go SDK: only files containing it are
    /// run to read their manifest.
    const MARKER: &'static [u8] = b"rutis-go-runtime:1";
    /// The plugin API of Go binaries this host runs, read from their
    /// manifests. The one definition: `rutis-host` uses it for the same
    /// check.
    const PLUGIN_API: u64 = 3;

    fn runtime_name(path: &Path) -> String {
        go_runtime_name(path)
    }

    fn launch(binary: &Path, project: &Path) -> Launcher {
        Launcher::new(binary).cwd(project).inherit_fd()
    }
}

/// The name of the Go runtime the binary at `path` runs as: `go-<its file
/// name>` ([`super::binary::runtime_name`]).
pub fn go_runtime_name(path: &Path) -> String {
    super::binary::runtime_name("go", path)
}

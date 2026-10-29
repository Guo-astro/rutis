//! Language runtimes: processes that run plugins written in another
//! language (Node with Cordis, feature `node`; Python, feature `python`;
//! Go, feature `go`; Bun, feature `bun`; Swift, feature `swift`),
//! whose plugins rutis-loader manages as rows.
//!
//! A runtime's session comes from a link, wherever the runtime runs:
//! [`LocalRuntime`] starts one on this machine (the local transport starts
//! the process, a link carries its session); [`RuntimePlugin::remote`] uses
//! one elsewhere, through the session [`RuntimeAccessPlugin`] provides from
//! a link to it. Either way the rows are the same.

// Inside the crate, runtime code names session items through this module.
#[allow(unused_imports)]
pub(crate) use crate::session::*;

mod access;
mod calls;
mod definitions;
mod local;
mod plugin;
mod process;
mod projection;
mod rows;
pub(crate) mod spawn;
#[cfg(feature = "testing")]
pub mod testing;
#[cfg(unix)]
pub(crate) mod unix;

/// Where a runtime process's standard streams go ([`Launcher::stdin`],
/// [`LocalRuntime::stdin`]).
pub use crate::pipe::Stdio;
pub use access::RuntimeAccessPlugin;
pub(crate) use access::{peer_session, Routed};
pub use calls::InFlight;
pub(crate) use definitions::read as read_definition;
pub use definitions::{mounter_key, returned, ForeignPlugin, PluginMounter};
pub use local::LocalRuntime;
pub use plugin::{Runtime, RuntimeHandle, RuntimePlugin, RuntimeState};
pub(crate) use process::calling as calling_process;
pub use process::{
    scoped_id, Host, HostLease, Launcher, Mount, PluginSchema, Process, RowSchema, ServiceEvents,
};
pub use projection::Projection;
pub use rows::{row_projection, row_projection_of, row_projection_with, RowService};

use std::sync::Arc;

use crate::session::{Connection, Dispatch, Error};

/// A session with a runtime that something else owns, such as a link to a
/// remote runtime. A [`RuntimePlugin::remote`] runs its rows on it.
pub trait RuntimeSession: Send + Sync + 'static {
    /// The session, ready.
    fn connection(&self) -> Connection;

    /// Route the runtime's calls into rutis (`host:<name>`, `service`,
    /// `events.*`, `rows.ended`) to `dispatch` while the returned guard lives.
    fn route(
        &self,
        dispatch: Arc<dyn Dispatch>,
    ) -> Result<Box<dyn std::any::Any + Send + Sync>, Error>;
}

/// The name of the Go runtime the binary at `path` runs as: its file name
/// without `.exe`, lowercased, with every character outside `[a-z0-9-]` as
/// `-`, after `go-` (`net.kit_v1.exe` -> `go-net-kit-v1`). It is the only
/// name users see for it.
#[cfg(feature = "go")]
pub fn go_runtime_name(path: &std::path::Path) -> String {
    binary_runtime_name("go", path)
}

/// The name of the Swift runtime the binary at `path` runs as, as
/// its file name without `.exe`, lowercased, with every character outside
/// `[a-z0-9-]` as `-`, after `swift-` (`Net.Kits_v1` -> `swift-net-kits-v1`).
#[cfg(feature = "swift")]
pub fn swift_runtime_name(path: &std::path::Path) -> String {
    binary_runtime_name("swift", path)
}

/// `<prefix>-<the file name>`, the name of a compiled program's runtime.
#[cfg(any(feature = "go", feature = "swift"))]
fn binary_runtime_name(prefix: &str, path: &std::path::Path) -> String {
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // By bytes: a name need not be ASCII, and `.exe` is.
    let exe = file.len() > 4 && file.as_bytes()[file.len() - 4..].eq_ignore_ascii_case(b".exe");
    let stem = match exe {
        true => &file[..file.len() - 4],
        false => &file[..],
    };
    let name: String = stem
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '-' => c,
            _ => '-',
        })
        .collect();
    format!("{prefix}-{name}")
}

/// The key the runtime session named `name` is provided under
/// (`RuntimeSession#gpu`).
pub fn runtime_session_key(name: &str) -> rutis::host::Key<dyn RuntimeSession> {
    rutis::host::Key::dynamic(name.to_owned())
}

#[cfg(all(test, any(feature = "go", feature = "swift")))]
mod tests {
    use std::path::Path;

    #[cfg(feature = "go")]
    #[test]
    fn go_runtime_names_come_from_file_names() {
        for (file, name) in [
            ("netkit", "go-netkit"),
            ("net.kit_v1.exe", "go-net-kit-v1"),
            ("NetKit.EXE", "go-netkit"),
            ("天气", "go---"),
            ("天气.exe", "go---"),
            ("a天", "go-a-"),
        ] {
            assert_eq!(super::go_runtime_name(Path::new(file)), name, "{file}");
        }
    }

    #[cfg(feature = "swift")]
    #[test]
    fn swift_runtime_names_come_from_file_names() {
        use super::swift_runtime_name;
        for (file, name) in [
            ("weather", "swift-weather"),
            ("Net.Kits_v1", "swift-net-kits-v1"),
        ] {
            assert_eq!(swift_runtime_name(Path::new(file)), name, "{file}");
        }
    }
}

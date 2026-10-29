//! What a plugin of the far end of a session declares
//! ([`SessionResolver`](super::SessionResolver)): the plugins its `mount`
//! reply listed, else `rows.schema` for each
//! (`docs/design-remote-session-2026-10-28.md` §4.3).
//!
//! A runtime without the plugin fails the row with [`Unavailable`]:
//! other rows are not affected, and an edit adding the row is not refused
//! because of it.

use std::path::Path;

use rutis_bridge::runtime::{Process, RowSchema};

use crate::error::Unavailable;

/// What one plugin declares on one runtime session.
#[derive(Clone)]
pub(crate) enum Described {
    Found(RowSchema),
    /// The runtime does not contain the plugin.
    Missing,
    /// Asking the runtime failed: why.
    Failed(String),
}

/// Read what `plugin` declares on `process`: from the plugins its `mount`
/// reply listed, else by asking (`rows.schema`).
pub(crate) async fn describe(process: &Process, plugin: &Path) -> Described {
    if let Some(plugins) = process.plugins() {
        return match plugins.get(plugin.to_string_lossy().as_ref()) {
            Some(found) => Described::Found(found.clone()),
            None => Described::Missing,
        };
    }
    match process.describe_row(plugin).await {
        Ok(found) => Described::Found(found),
        // The runtime looked and found no such plugin.
        Err(rutis_bridge::session::Error::Remote { name, .. }) if name == "NotFound" => {
            Described::Missing
        }
        Err(error) => Described::Failed(error.to_string()),
    }
}

/// The runtime `runtime` does not contain `plugin`.
pub(crate) fn missing(runtime: &str, plugin: &str) -> Unavailable {
    Unavailable(format!(
        "runtime {runtime}: this connection does not contain plugin {plugin}"
    ))
}

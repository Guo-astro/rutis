//! The plugins a far rutis kernel may run here through a session
//! ([`SessionHandle::run`](crate::SessionHandle::run) there): the node side
//! of the session answers `mount` and `rows.*` from the catalog its host
//! provides at [`catalog_key`](crate::catalog_key) (rutis-loader: its
//! loader's). Names are looked up in it; files and absolute paths are not.

use std::collections::HashMap;
use std::sync::Arc;

use rutis::host::{BoxFuture, PluginFactory};
use serde_json::Value as Json;

/// The plugins installed here that a far kernel may run, with what each
/// declares. Looking one up may take a while (a loader's resolvers).
pub trait PluginCatalog: Send + Sync + 'static {
    fn find<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<Installed>>;
}

/// An installed plugin: what it declares, and how to build it.
#[derive(Clone)]
pub struct Installed {
    /// The JSON Schema of its configuration.
    pub schema: Option<Json>,
    pub version: Option<String>,
    pub factory: Arc<dyn PluginFactory<Json>>,
}

impl Installed {
    /// `factory`, declaring no schema and no version.
    pub fn new(factory: impl PluginFactory<Json>) -> Self {
        Self {
            schema: None,
            version: None,
            factory: Arc::new(factory),
        }
    }

    pub fn schema(mut self, schema: Json) -> Self {
        self.schema = Some(schema);
        self
    }

    pub fn version(mut self, version: &str) -> Self {
        self.version = Some(version.to_owned());
        self
    }
}

/// A catalog from a fixed list.
#[derive(Default)]
pub struct StaticCatalog {
    entries: HashMap<String, Installed>,
}

impl StaticCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, name: &str, installed: Installed) -> Self {
        self.entries.insert(name.to_owned(), installed);
        self
    }
}

impl PluginCatalog for StaticCatalog {
    fn find<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<Installed>> {
        let found = self.entries.get(name).cloned();
        Box::pin(async move { found })
    }
}

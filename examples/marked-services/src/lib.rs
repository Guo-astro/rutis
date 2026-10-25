//! A Rust service the rows of every language use by name: `Calendar` is
//! marked `#[rutis::service]` and provided as usual. There is no build.rs,
//! no list of exports and no `HostDispatch`; this crate depends on `rutis`
//! alone.
//!
//! ```ignore
//! root.provide(Calendar::new())?;   // TypeScript, Python, Go rows: ctx.use('calendar')
//! ```

use std::sync::Mutex;

use rutis::host::BoxFuture;
use rutis::service;
use serde::{Deserialize, Serialize};

/// A calendar entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub title: String,
    /// Milliseconds since the Unix epoch.
    pub starts_at: u64,
}

#[derive(Debug, Serialize, thiserror::Error)]
pub enum CalendarError {
    #[error("no entry {id}")]
    NotFound { id: usize },
}

/// The application's calendar.
#[derive(Default)]
pub struct Calendar {
    entries: Mutex<Vec<Entry>>,
}

#[service]
impl Calendar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an entry; its id.
    pub async fn add(&self, entry: Entry) -> usize {
        let mut entries = self.entries.lock().unwrap();
        entries.push(entry);
        entries.len() - 1
    }

    /// The entry `id`.
    pub fn get(&self, id: usize) -> Result<Entry, CalendarError> {
        self.entries
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or(CalendarError::NotFound { id })
    }

    /// Call `visit` with each entry, in order; how many there were.
    pub async fn each(
        &self,
        visit: impl Fn(Entry) -> BoxFuture<'static, rutis::Result>,
    ) -> rutis::Result<usize> {
        let entries = self.entries.lock().unwrap().clone();
        for entry in &entries {
            visit(entry.clone()).await?;
        }
        Ok(entries.len())
    }
}

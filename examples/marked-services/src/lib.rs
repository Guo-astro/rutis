//! A Rust service the rows of every language use by name: `Calendar` is
//! marked `#[rutis::service]` and provided as usual. There is no build.rs,
//! no list of exports and no `HostDispatch`; this crate depends on `rutis`
//! alone.
//!
//! ```ignore
//! root.provide(Calendar::new())?;   // TypeScript, Python, Go rows: ctx.use('calendar')
//! ```

use std::collections::HashMap;
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

/// How often a planned entry repeats.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    Never,
    Daily,
    Weekly,
}

/// A reminder before a planned entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reminder {
    pub minutes_before: u16,
    /// Off when left out.
    #[serde(default)]
    pub sound: bool,
    pub note: Option<String>,
}

/// What [`Calendar::plan`] was given.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub title: String,
    pub all_day: bool,
    pub hours: f64,
    pub repeat: Repeat,
    pub reminders: Vec<Reminder>,
    pub window: Option<(u8, u8)>,
    pub labels: HashMap<String, String>,
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
    plans: Mutex<Vec<Plan>>,
    sources: Mutex<Vec<String>>,
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

    /// Plan an entry; its number. Its parameters take one of each kind of
    /// value a service is given, so that the testing tools' contract
    /// checks can be compared with this service's own.
    #[allow(clippy::too_many_arguments)]
    pub fn plan(
        &self,
        title: String,
        all_day: bool,
        hours: f64,
        repeat: Repeat,
        reminders: Vec<Reminder>,
        window: Option<(u8, u8)>,
        labels: HashMap<String, String>,
    ) -> usize {
        let mut plans = self.plans.lock().unwrap();
        plans.push(Plan {
            title,
            all_day,
            hours,
            repeat,
            reminders,
            window,
            labels,
        });
        plans.len()
    }

    /// Add a plugin that brings entries from elsewhere, by name; how many
    /// there are.
    pub fn add_source(&self, name: String, _source: rutis::Definition) -> usize {
        let mut sources = self.sources.lock().unwrap();
        sources.push(name);
        sources.len()
    }
}

//! What the calendar's methods take and return.

mod error;

use std::sync::Arc;

use rutis::{BoxFuture, CordisError};
use serde::{Deserialize, Serialize};

pub use error::CalendarError;

/// An entry of the calendar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: u64,
    pub title: String,
    /// Milliseconds since the Unix epoch.
    pub starts_at: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub kind: Kind,
    pub note: Option<String>,
}

/// An entry to add: an [`Entry`] without its id.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewEntry {
    pub title: String,
    pub starts_at: u64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Meeting,
    Reminder,
    AllDay,
}

/// What changed in the calendar, as its subscribers hear it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Change {
    Added { entry: Entry },
    Removed { id: u64 },
}

/// A subscriber: called for each change, and the change is handled once
/// every subscriber's future resolved; one failing fails the change.
pub type Handler = Arc<dyn Fn(Change) -> BoxFuture<'static, Result<(), CordisError>> + Send + Sync>;

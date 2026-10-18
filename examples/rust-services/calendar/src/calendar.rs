use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rutis::{BoxFuture, CordisError, Ctx, Effect};

use crate::model::{CalendarError, Change, Entry, Handler, NewEntry};

/// The calendar: entries, and subscribers told of each change.
#[derive(Default)]
pub struct Calendar {
    entries: Mutex<BTreeMap<u64, Entry>>,
    next: AtomicU64,
    subscribers: Arc<Mutex<Vec<(u64, Handler)>>>,
    capacity: Option<usize>,
}

impl Calendar {
    pub fn new() -> Self {
        Self::default()
    }

    /// A calendar holding at most `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: Some(capacity),
            ..Self::default()
        }
    }

    /// Add an entry; returns it with its id.
    pub fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> {
        if entry.title.trim().is_empty() {
            return Err(CalendarError::Invalid("an entry needs a title".into()));
        }
        let mut entries = self.entries.lock().unwrap();
        if self
            .capacity
            .is_some_and(|capacity| entries.len() >= capacity)
        {
            return Err(CalendarError::Full);
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = Entry {
            id,
            title: entry.title,
            starts_at: entry.starts_at,
            tags: entry.tags,
            kind: entry.kind,
            note: None,
        };
        entries.insert(id, entry.clone());
        Ok(entry)
    }

    /// The entry `id`.
    pub async fn get(&self, id: u64) -> Result<Entry, CalendarError> {
        self.entries
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or(CalendarError::NotFound { id })
    }

    /// The entries, those with `tag` only when it is given.
    pub async fn list(&self, tag: Option<String>) -> Vec<Entry> {
        self.entries
            .lock()
            .unwrap()
            .values()
            .filter(|entry| tag.as_ref().is_none_or(|tag| entry.tags.contains(tag)))
            .cloned()
            .collect()
    }

    pub fn rename(&self, id: u64, title: &str) -> Result<Entry, CalendarError> {
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.get_mut(&id).ok_or(CalendarError::NotFound { id })?;
        entry.title = title.to_owned();
        Ok(entry.clone())
    }

    pub fn remove(&self, id: u64) -> Result<(), CalendarError> {
        self.entries
            .lock()
            .unwrap()
            .remove(&id)
            .map(|_| ())
            .ok_or(CalendarError::NotFound { id })
    }

    /// The largest id the calendar can give.
    pub fn last_id(&self) -> u64 {
        u64::MAX
    }

    /// Tell `handler` of every change from now on, until `ctx`'s plugin
    /// unloads or [`Calendar::unsubscribe`]; returns the subscription.
    pub fn subscribe(&self, ctx: &Ctx, handler: Handler) -> Result<u64, CordisError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.subscribers.lock().unwrap().push((id, handler));
        let subscribers = self.subscribers.clone();
        ctx.effect(move || {
            Effect::Disposer(Box::new(move || {
                subscribers
                    .lock()
                    .unwrap()
                    .retain(|(known, _)| *known != id);
                Ok(())
            }))
        })?;
        Ok(id)
    }

    pub fn unsubscribe(&self, subscription: u64) -> bool {
        let mut subscribers = self.subscribers.lock().unwrap();
        let before = subscribers.len();
        subscribers.retain(|(id, _)| *id != subscription);
        subscribers.len() != before
    }

    /// Visit every entry in order, each visit finishing before the next;
    /// returns how many were visited.
    pub async fn each(
        &self,
        visit: impl Fn(Entry) -> BoxFuture<'static, Result<(), CordisError>> + Send + Sync,
    ) -> Result<usize, CordisError> {
        let entries: Vec<Entry> = self.entries.lock().unwrap().values().cloned().collect();
        for entry in &entries {
            visit(entry.clone()).await?;
        }
        Ok(entries.len())
    }

    /// Hand `change` to every subscriber in turn; done once each of them
    /// handled it, failed when one of them failed.
    pub async fn publish(&self, change: Change) -> Result<(), CordisError> {
        let handlers: Vec<Handler> = self
            .subscribers
            .lock()
            .unwrap()
            .iter()
            .map(|(_, handler)| handler.clone())
            .collect();
        for handler in handlers {
            handler(change.clone()).await?;
        }
        Ok(())
    }

    /// How many subscribers there are.
    pub fn subscribers(&self) -> usize {
        self.subscribers.lock().unwrap().len()
    }
}

/// One session's notes: a service each instance of a session has its own
/// of.
pub struct Notes {
    owner: String,
    lines: Mutex<Vec<String>>,
}

impl Notes {
    pub fn new(owner: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            lines: Mutex::new(Vec::new()),
        }
    }

    /// Whose notes these are.
    pub fn owner(&self) -> String {
        self.owner.clone()
    }

    pub fn write(&self, line: String) -> usize {
        let mut lines = self.lines.lock().unwrap();
        lines.push(line);
        lines.len()
    }

    pub fn read(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}
